#[cfg(not(target_arch = "wasm32"))]
mod home;
#[cfg(not(target_arch = "wasm32"))]
mod secrets;
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(not(target_arch = "wasm32"))]
mod worker;

use meatpi_core::Sample;
#[cfg(not(target_arch = "wasm32"))]
use meatpi_core::KIA_EV6;
use slint::SharedString;
#[cfg(not(target_arch = "wasm32"))]
use slint::{ComponentHandle, Model, ModelRc, VecModel};
#[cfg(not(target_arch = "wasm32"))]
use std::rc::Rc;
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use worker::{Command, Event, Source, Status, Worker};

slint::include_modules!();

#[cfg(not(target_arch = "wasm32"))]
const MAX_LOG_LINES: usize = 300;

#[cfg(not(target_arch = "wasm32"))]
pub fn run() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.set_profile_name(KIA_EV6.name.into());
    ui.set_log_lines(ModelRc::from(Rc::new(VecModel::<SharedString>::default())));

    // Bridge worker-thread events onto the Slint event loop. Only the `Weak`
    // handle and the event itself cross the thread boundary, which is what
    // keeps this closure `Send`.
    let weak = ui.as_weak();
    let worker = Rc::new(Worker::spawn(&KIA_EV6, move |event| {
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade() {
                handle_event(&ui, event);
            }
        });
    }));

    ui.on_connect_requested({
        let ui = ui.as_weak();
        let worker = worker.clone();
        move || {
            let Some(ui) = ui.upgrade() else { return };
            let source = if ui.get_simulate() {
                Source::Simulator
            } else if ui.get_remote() {
                Source::MeatHandler {
                    url: ui.get_remote_url().trim().to_string(),
                    user: ui.get_remote_user().trim().to_string(),
                    pass: ui.get_remote_pass().to_string(),
                    remember: ui.get_remember(),
                }
            } else {
                let port = ui.get_port().trim().parse::<u16>().unwrap_or({
                    ui.set_status("Port must be a number, using 35000".into());
                    meatpi_core::DEFAULT_PORT
                });
                Source::Dongle {
                    host: ui.get_host().trim().to_string(),
                    port,
                }
            };
            worker.send(Command::Connect(source));
        }
    });

    ui.on_disconnect_requested({
        let worker = worker.clone();
        move || worker.send(Command::Disconnect)
    });

    ui.on_forget_requested({
        let ui = ui.as_weak();
        move || {
            secrets::forget();
            if let Some(ui) = ui.upgrade() {
                ui.set_remote_pass("".into());
                ui.set_has_saved_login(false);
            }
        }
    });

    // Home: the Home Assistant app on Android (offering the Play Store or the
    // browser if it is missing), the browser everywhere else.
    ui.on_home_requested({
        let ui = ui.as_weak();
        move || {
            let Some(ui) = ui.upgrade() else { return };
            match home::open_app(&home::ha_url(&ui.get_remote_url())) {
                Ok(home::Launch::Opened) => {}
                Ok(home::Launch::NotInstalled) => ui.set_ha_prompt(true),
                Err(e) => push_log(&ui, format!("Home Assistant: {e}")),
            }
        }
    });
    ui.on_ha_install_requested({
        let ui = ui.as_weak();
        move || {
            let Some(ui) = ui.upgrade() else { return };
            ui.set_ha_prompt(false);
            if let Err(e) = home::open_store() {
                push_log(&ui, format!("Play Store: {e}"));
            }
        }
    });
    ui.on_ha_browser_requested({
        let ui = ui.as_weak();
        move || {
            let Some(ui) = ui.upgrade() else { return };
            ui.set_ha_prompt(false);
            if let Err(e) = home::open_url(&home::ha_url(&ui.get_remote_url())) {
                push_log(&ui, format!("Home Assistant: {e}"));
            }
        }
    });

    ui.on_command_entered({
        let worker = worker.clone();
        move |cmd| {
            let cmd = cmd.trim().to_string();
            if !cmd.is_empty() {
                worker.send(Command::Raw(cmd));
            }
        }
    });

    // `MEATPI_SIM=1` boots straight into the simulator, which is how you work
    // on the UI without the car. `MEATPI_HOST` overrides the dongle address
    // and `MEATPI_DIRECT=1` starts in direct-dongle mode instead of
    // MeatHandler. `MEATHANDLER_URL`, `MEATHANDLER_USER` and
    // `MEATHANDLER_PASS` fill in the MeatHandler login. `MEATPI_CONNECT=1`
    // connects on launch instead of waiting for a click.
    let enabled = |key: &str| std::env::var(key).is_ok_and(|v| v != "0");

    // A login saved by an earlier run means we can connect straight away.
    let saved = secrets::load();
    if let Some(login) = &saved {
        ui.set_remote_url(login.url.clone().into());
        ui.set_remote_user(login.user.clone().into());
        ui.set_remote_pass(login.pass.clone().into());
        ui.set_has_saved_login(true);
    }

    if let Ok(host) = std::env::var("MEATPI_HOST") {
        ui.set_host(host.into());
    }
    if enabled("MEATPI_DIRECT") {
        ui.set_remote(false);
    }
    if let Ok(url) = std::env::var("MEATHANDLER_URL") {
        ui.set_remote_url(url.into());
    }
    if let Ok(user) = std::env::var("MEATHANDLER_USER") {
        ui.set_remote_user(user.into());
    }
    if let Ok(pass) = std::env::var("MEATHANDLER_PASS") {
        ui.set_remote_pass(pass.into());
    }
    if enabled("MEATPI_SIM") {
        ui.set_simulate(true);
    }
    let auto_connect = saved.is_some() && ui.get_remote();
    if enabled("MEATPI_SIM") || enabled("MEATPI_CONNECT") || auto_connect {
        ui.invoke_connect_requested();
    }

    ui.run()
}

#[cfg(not(target_arch = "wasm32"))]
fn handle_event(ui: &AppWindow, event: Event) {
    match event {
        Event::Status(status, message) => {
            ui.set_link(match status {
                Status::Disconnected => LinkState::Disconnected,
                Status::Connecting => LinkState::Connecting,
                Status::Live => LinkState::Live,
                Status::Failed => LinkState::Failed,
            });
            ui.set_status(message.into());
            if status == Status::Disconnected || status == Status::Failed {
                ui.set_data(Telemetry::default());
            }
        }

        Event::Sample(sample) => ui.set_data(to_telemetry(&sample)),

        Event::Log(line) => push_log(ui, line),

        Event::LoginSaved(saved) => ui.set_has_saved_login(saved),
    }
}

/// The log model lives on the window itself, so nothing extra has to be shared
/// between callbacks.
#[cfg(not(target_arch = "wasm32"))]
fn push_log(ui: &AppWindow, line: String) {
    let model = ui.get_log_lines();
    let Some(lines) = model.as_any().downcast_ref::<VecModel<SharedString>>() else {
        return;
    };
    if lines.row_count() >= MAX_LOG_LINES {
        lines.remove(0);
    }
    lines.push(line.into());
}

/// "8s ago", "3m ago", ... for status lines.
pub(crate) fn ago(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86_399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

/// The dongle posts every 10 s while the car is awake. Anything much older
/// means it has gone to sleep.
pub(crate) const FRESH_FOR: Duration = Duration::from_secs(90);

/// Status line for a MeatHandler reading of the given age.
pub(crate) fn remote_status(age: Option<Duration>) -> String {
    match age {
        None => "MeatHandler · no data from the car yet".into(),
        Some(a) if a <= FRESH_FOR => format!("Live · {}", ago(a)),
        Some(a) => format!("Car asleep · last update {}", ago(a)),
    }
}

/// Format a value, or an em dash when the PID did not answer. Keeping this on
/// the Rust side means the UI never has to invent a placeholder number.
fn num(value: Option<f64>, decimals: usize) -> SharedString {
    match value {
        Some(v) => SharedString::from(format!("{v:.decimals$}")),
        None => SharedString::from("—"),
    }
}

fn to_telemetry(s: &Sample) -> Telemetry {
    let min_t = s.get("batt_min_temp");
    let max_t = s.get("batt_max_temp");
    let temp_text = match (min_t, max_t) {
        (Some(lo), Some(hi)) => SharedString::from(format!("{lo:.0} / {hi:.0}")),
        _ => SharedString::from("—"),
    };

    let cell_range_text = match (s.get("min_cell_v"), s.get("max_cell_v")) {
        (Some(lo), Some(hi)) => SharedString::from(format!("{lo:.2} / {hi:.2}")),
        _ => SharedString::from("—"),
    };

    let unanswered = s.stale.len();
    let updated_text = if unanswered == 0 {
        SharedString::from("all readings present")
    } else {
        SharedString::from(format!("{unanswered} reading(s) missing"))
    };

    let charging = s.is_charging();
    let charge_text = SharedString::from(match (charging, s.is_plugged()) {
        (Some(true), _) => "Charging",
        (_, Some(true)) => "Plugged in, not charging",
        (_, Some(false)) => "Not plugged in",
        (Some(false), None) => "Not charging",
        (None, None) => "—",
    });
    let charge_rate_text = match (charging, s.power_kw()) {
        (Some(true), Some(kw)) => num(Some(-kw), 1),
        _ => SharedString::from("—"),
    };

    Telemetry {
        charge_text,
        charge_rate_text,
        charging: charging == Some(true),
        soc: s.soc().unwrap_or(0.0) as f32,
        soc_known: s.soc().is_some(),
        soc_text: num(s.soc(), 1),
        power_text: num(s.power_kw(), 1),
        pack_voltage_text: num(s.get("pack_voltage"), 1),
        pack_current_text: num(s.get("pack_current"), 1),
        soh_text: num(s.get("soh"), 1),
        temp_text,
        cell_range_text,
        aux_text: num(s.get("aux_voltage"), 1),
        regen_limit_text: num(s.get("max_regen_kw"), 1),
        power_limit_text: num(s.get("max_power_kw"), 1),
        cell_dev_text: num(s.get("cell_v_deviation"), 2),
        updated_text,
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: slint::android::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Debug),
    );
    secrets::init(&app);
    home::init(app.clone());
    slint::android::init(app).expect("failed to initialise the Android backend");
    if let Err(e) = run() {
        log::error!("app exited: {e}");
    }
}
