//! Command-line probe for verifying the dongle and the decode offsets.
//!
//!     cargo run -p meatpi-core --example probe -- 10.43.1.39:35000
//!     cargo run -p meatpi-core --example probe -- --sim
//!
//! Prints the adapter identity, the raw response to each request, and the
//! decoded values, so you can check the byte offsets in `profile.rs` against
//! what the car actually reports. See docs/PROTOCOL.md.

use meatpi_core::{Client, DEFAULT_PORT, KIA_EV6};
use std::time::Duration;

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: probe <host[:port]> | --sim");
        std::process::exit(2);
    });

    let mut client = if arg == "--sim" {
        println!("connecting to the built-in simulator");
        Client::connect_simulated(&KIA_EV6)
    } else {
        let (host, port) = match arg.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse().unwrap_or(DEFAULT_PORT)),
            None => (arg.clone(), DEFAULT_PORT),
        };
        println!("connecting to {host}:{port}");
        Client::connect(&host, port, &KIA_EV6, Duration::from_secs(5))
    };

    let client = match client.as_mut() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("connect failed: {e}");
            eprintln!("\nIf the car is off, the dongle sleeps and drops off the");
            eprintln!("network. Wake the car and try again.");
            std::process::exit(1);
        }
    };

    println!("profile: {}\n", KIA_EV6.name);

    for cmd in ["ATI", "ATRV"] {
        match client.raw_command(cmd) {
            Ok(reply) => println!("{cmd:<8} {}", reply.replace('\n', " ").trim()),
            Err(e) => println!("{cmd:<8} error: {e}"),
        }
    }

    println!("\n--- raw responses ---");
    for req in KIA_EV6.requests {
        // Address the right ECU before asking, exactly as the poller does.
        let _ = client.raw_command(&format!("ATSH{}", req.header));
        match client.raw_command(req.cmd) {
            Ok(reply) => {
                let flat: Vec<&str> = reply.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
                println!("{} @ {} -> {}", req.cmd, req.header, flat.join(" | "));
            }
            Err(e) => println!("{} @ {} -> error: {e}", req.cmd, req.header),
        }
    }

    println!("\n--- decoded ---");
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
