//! Fetch the car's latest values from MeatHandler, without the UI.
//!
//!     MEATHANDLER_PASS=... cargo run -p meatpi-core --features remote --example remote_probe
//!
//! `MEATHANDLER_URL` and `MEATHANDLER_USER` default to the home server.

use meatpi_core::remote::{RemoteClient, DEFAULT_URL};
use std::time::Duration;

fn main() {
    let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    let url = var("MEATHANDLER_URL", DEFAULT_URL);
    let client = RemoteClient::new(
        &url,
        &var("MEATHANDLER_USER", "enoch"),
        &var("MEATHANDLER_PASS", ""),
        Duration::from_secs(10),
    );
    match client.fetch() {
        Ok(snap) => {
            println!("last update: {:?} ago", snap.age.unwrap_or_default());
            let mut rows: Vec<_> = snap.sample.iter().collect();
            rows.sort_by(|a, b| a.0.cmp(b.0));
            for (k, v) in rows {
                println!("  {k:<18} {v}");
            }
            if !snap.sample.stale.is_empty() {
                println!("missing: {}", snap.sample.stale.join(", "));
            }
        }
        Err(e) => {
            eprintln!("{}: {e}", client.url());
            std::process::exit(1);
        }
    }
}
