//! Read the car over BLE instead of Wi-Fi, using the same ELM327 and decoding
//! stack as the TCP path.
//!
//!     ./scripts/macos-ble-bundle.sh ble_probe
//!
//! Running it as a bare binary on macOS is killed by TCC — CoreBluetooth needs
//! an app bundle with NSBluetoothAlwaysUsageDescription.

use meatpi_core::ble::{BleTransport, NAME_PREFIX};
use meatpi_core::{Client, KIA_EV6};
use std::time::Duration;

fn main() {
    println!("scanning for {NAME_PREFIX}* over BLE…");

    let transport = match BleTransport::connect(
        NAME_PREFIX,
        Duration::from_secs(8),
        Duration::from_secs(3),
    ) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("BLE connect failed: {e}");
            std::process::exit(1);
        }
    };
    println!("BLE link up; starting ELM327 session…");

    let mut client = match Client::with_transport(Box::new(transport), &KIA_EV6) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ELM327 init failed over BLE: {e}");
            std::process::exit(1);
        }
    };

    match client.raw_command("ATI") {
        Ok(reply) => println!("ATI      {}", reply.replace('\n', " ").trim()),
        Err(e) => println!("ATI      error: {e}"),
    }

    println!("\n--- decoded over BLE ---");
    let sample = client.poll();
    if sample.is_empty() {
        println!("nothing decoded; every request failed");
    } else {
        let mut rows: Vec<(&str, f64)> = sample.iter().map(|(k, v)| (*k, *v)).collect();
        rows.sort_by_key(|(k, _)| *k);
        for (key, value) in rows {
            println!("  {key:<20} {value:>10.2}");
        }
        if let Some(p) = sample.power_kw() {
            println!("  {:<20} {p:>10.2}  (derived)", "power_kw");
        }
    }
    if !sample.stale.is_empty() {
        println!("\nunanswered: {:?}", sample.stale);
    }
}
