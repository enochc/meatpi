//! Discover the WiCAN Pro over BLE and dump its GATT layout.
//!
//!     cargo run -p meatpi-core --features ble --example ble_scan
//!
//! The firmware exposes an ESP-IDF "SPP-like" service (see `main/ble.c` in
//! wican-fw):
//!
//!   service 0xFEE0
//!     char 0xFEE1  notify   — data from the adapter
//!     char 0xFEE3  write    — commands to the adapter
//!
//! Both characteristics are declared `ENC_MITM`, so the link must be encrypted
//! and authenticated before they can be used. That is what the passkey is for,
//! and the OS — not this program — prompts for it.

use btleplug::api::bleuuid::{uuid_from_u16, BleUuid};
use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::Manager;
use std::time::Duration;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The firmware's `main` branch declares 0xFEE0/1/3, but the Pro advertises
    // 0xFFF0 — the layout most ELM327 BLE dongles use. Accept either.
    let svc_spp: Uuid = uuid_from_u16(0xFEE0);
    let svc_fff0: Uuid = uuid_from_u16(0xFFF0);
    let chr_notify: Uuid = uuid_from_u16(0xFEE1);
    let chr_write: Uuid = uuid_from_u16(0xFEE3);

    let manager = Manager::new().await?;
    let adapter = manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .ok_or("no Bluetooth adapter found")?;

    println!("scanning for 10 s on {}…\n", adapter.adapter_info().await?);
    adapter.start_scan(ScanFilter::default()).await?;
    tokio::time::sleep(Duration::from_secs(10)).await;
    adapter.stop_scan().await?;

    let peripherals = adapter.peripherals().await?;
    if peripherals.is_empty() {
        println!("no BLE devices seen at all — is Bluetooth permission granted?");
        return Ok(());
    }

    let mut candidate = None;
    println!("{:<28} {:>5}  services", "name", "rssi");
    println!("{}", "-".repeat(60));

    for p in &peripherals {
        let Some(props) = p.properties().await? else {
            continue;
        };
        let name = props.local_name.unwrap_or_else(|| "(unnamed)".into());
        let rssi = props
            .rssi
            .map(|r| r.to_string())
            .unwrap_or_else(|| "?".into());

        // The Pro advertises as `WiC_<ble-mac>`; other OBD dongles in range
        // also advertise 0xFFF0, so the name is what identifies ours.
        let lower = name.to_lowercase();
        let interesting = props.services.contains(&svc_spp)
            || lower.starts_with("wic")
            || lower.contains("wican")
            || lower.contains("meatpi");

        // Unnamed strangers with no services are just noise.
        if !interesting && props.services.is_empty() && name == "(unnamed)" {
            continue;
        }

        let svcs: Vec<String> = props.services.iter().map(|u| u.to_short_string()).collect();
        println!(
            "{}{:<28} {:>5}  {}",
            if interesting { "* " } else { "  " },
            name,
            rssi,
            if svcs.is_empty() {
                "(none advertised)".to_string()
            } else {
                svcs.join(", ")
            }
        );

        if interesting && candidate.is_none() {
            candidate = Some(p.clone());
        }
    }

    let Some(dongle) = candidate else {
        println!("\nNo WiCAN found. It only advertises while awake — if the car");
        println!("is off, the firmware calls ble_disable() and deep sleeps.");
        return Ok(());
    };

    let name = dongle
        .properties()
        .await?
        .and_then(|p| p.local_name)
        .unwrap_or_else(|| "(unnamed)".into());
    println!("\nfound {name}; connecting…");

    dongle.connect().await?;
    dongle.discover_services().await?;
    println!("connected. GATT layout:\n");

    for service in dongle.services() {
        let tag = if service.uuid == svc_spp || service.uuid == svc_fff0 {
            " <- data service"
        } else {
            ""
        };
        println!("  service {}{}", service.uuid.to_short_string(), tag);
        for c in &service.characteristics {
            let role = if c.uuid == chr_notify {
                "  <- notify: adapter -> us"
            } else if c.uuid == chr_write {
                "  <- write: us -> adapter"
            } else {
                ""
            };
            println!(
                "    char {} {:?}{}",
                c.uuid.to_short_string(),
                c.properties,
                role
            );
        }
    }

    dongle.disconnect().await?;
    println!("\ndisconnected.");
    Ok(())
}

