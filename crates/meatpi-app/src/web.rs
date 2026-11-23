//! The dashboard compiled to WebAssembly and served by MeatHandler at `/app/`.
//!
//! The native app reads MeatHandler on a worker thread with a blocking HTTP
//! client. A browser has neither threads nor sockets for that, so here a Slint
//! timer asks the browser to `fetch("/api/latest")` every few seconds and the
//! reply goes through the same parser and the same `to_telemetry` as native.
//!
//! The page is served by MeatHandler itself, so the request is same-origin and
//! the browser attaches the dashboard's Basic auth on its own. The web build
//! never sees or stores a password.

use crate::{remote_status, to_telemetry, AppWindow, LinkState};
use meatpi_core::{remote::parse_latest, Sample, KIA_EV6};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const LATEST_URL: &str = "/api/latest";

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);
    if let Err(e) = run_web() {
        log::error!("dashboard failed to start: {e}");
    }
}

fn js_err(v: JsValue) -> String {
    v.as_string().unwrap_or_else(|| format!("{v:?}"))
}

async fn fetch_latest() -> Result<String, String> {
    let window = web_sys::window().ok_or("no browser window")?;
    let resp = JsFuture::from(window.fetch_with_str(LATEST_URL)).await.map_err(js_err)?;
    let resp: web_sys::Response = resp.dyn_into().map_err(js_err)?;
    if !resp.ok() {
        return Err(match resp.status() {
            401 => "not logged in to MeatHandler (reload the page)".into(),
            code => format!("MeatHandler returned HTTP {code}"),
        });
    }
    let text = JsFuture::from(resp.text().map_err(js_err)?).await.map_err(js_err)?;
    text.as_string().ok_or_else(|| "reply was not text".into())
}

fn run_web() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    // `?controls` shows the native connection controls. They do nothing in
    // the browser; it exists to check the phone layout in a headless browser.
    let show_controls = web_sys::window()
        .and_then(|w| w.location().search().ok())
        .is_some_and(|q| q.contains("controls"));
    ui.set_web_mode(!show_controls);
    ui.set_profile_name(KIA_EV6.name.into());
    ui.set_link(LinkState::Connecting);
    ui.set_status("Loading from MeatHandler…".into());

    let latest = Rc::new(RefCell::new(Sample::default()));
    let in_flight = Rc::new(std::cell::Cell::new(false));

    let poll = {
        let weak = ui.as_weak();
        move || {
            // A slow reply must not stack up requests behind it.
            if in_flight.replace(true) {
                return;
            }
            let weak = weak.clone();
            let latest = latest.clone();
            let in_flight = in_flight.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = fetch_latest().await;
                in_flight.set(false);
                let Some(ui) = weak.upgrade() else { return };
                match result.and_then(|body| parse_latest(&body).map_err(|e| e.to_string())) {
                    Ok(snap) => {
                        latest.borrow_mut().merge_from(&snap.sample);
                        ui.set_data(to_telemetry(&latest.borrow()));
                        ui.set_link(LinkState::Live);
                        ui.set_status(remote_status(snap.age).into());
                    }
                    // Keep the last values on screen; just flag the problem.
                    Err(e) => {
                        ui.set_link(LinkState::Failed);
                        ui.set_status(e.into());
                    }
                }
            });
        }
    };

    poll();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, POLL_INTERVAL, poll);
    // On the web the event loop outlives this function, so keep the window
    // and timer alive for the life of the page.
    std::mem::forget(timer);
    std::mem::forget(ui.clone_strong());
    ui.run()
}
