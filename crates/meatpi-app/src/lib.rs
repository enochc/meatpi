mod worker;

use meatpi_core::{Sample, KIA_EV6};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::rc::Rc;
use worker::{Command, Event, Status, Worker};

slint::include_modules!();

const MAX_LOG_LINES: usize = 300;

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
            let port = ui.get_port().trim().parse::<u16>().unwrap_or({
                ui.set_status("Port must be a number, using 35000".into());
                meatpi_core::DEFAULT_PORT
            });
            worker.send(Command::Connect {
                host: ui.get_host().trim().to_string(),
                port,
                simulate: ui.get_simulate(),
            });
        }
    });

    ui.on_disconnect_requested({
        let worker = worker.clone();
        move || worker.send(Command::Disconnect)
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
    // on the UI without the car. `MEATPI_HOST` overrides the address, and
    // `MEATPI_CONNECT=1` connects on launch instead of waiting for a click.
    let enabled = |key: &str| std::env::var(key).is_ok_and(|v| v != "0");

    if let Ok(host) = std::env::var("MEATPI_HOST") {
        ui.set_host(host.into());
    }
    if enabled("MEATPI_SIM") {
        ui.set_simulate(true);
    }
    if enabled("MEATPI_SIM") || enabled("MEATPI_CONNECT") {
        ui.invoke_connect_requested();
    }

    ui.run()
}

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
    }
}

/// The log model lives on the window itself, so nothing extra has to be shared
/// between callbacks.
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
        SharedString::from("all requests answered")
    } else {
        SharedString::from(format!("{unanswered} request(s) unanswered"))
    };

    Telemetry {
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
    slint::android::init(app).expect("failed to initialise the Android backend");
    if let Err(e) = run() {
        log::error!("app exited: {e}");
    }
}
