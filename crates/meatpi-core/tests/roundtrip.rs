//! Drives the full stack — ELM327 framing, hex reassembly, marker search and
//! decoding — against the built-in simulator.

use meatpi_core::{Client, KIA_EV6};

#[test]
fn simulator_produces_a_plausible_sample() {
    let mut client = Client::connect_simulated(&KIA_EV6).expect("simulator should connect");
    let sample = client.poll();

    assert!(sample.stale.is_empty(), "unanswered: {:?}", sample.stale);

    let soc = sample.soc().expect("SOC should decode");
    assert!((5.0..=100.0).contains(&soc), "soc out of range: {soc}");

    // The EV6 runs an 800 V pack.
    let voltage = sample.get("pack_voltage").expect("pack voltage");
    assert!((600.0..=850.0).contains(&voltage), "voltage: {voltage}");

    let soh = sample.get("soh").expect("state of health");
    assert!((soh - 98.9).abs() < 0.05, "soh: {soh}");

    let aux = sample.get("aux_voltage").expect("12 V battery");
    assert!((aux - 14.1).abs() < 1e-6, "aux: {aux}");
    assert_eq!(sample.get("batt_max_temp"), Some(35.0));

    // Cell voltage must be consistent with the pack: 192 cells in series.
    let cell = sample.get("max_cell_v").expect("highest cell");
    let implied_cells = voltage / cell;
    assert!(
        (180.0..=200.0).contains(&implied_cells),
        "{cell} V cells imply {implied_cells} cells in series"
    );

    // Power must follow from voltage and current, and be signed correctly.
    let power = sample.power_kw().expect("power");
    let expected = voltage * sample.get("pack_current").unwrap() / 1000.0;
    assert!((power - expected).abs() < 1e-6);
}

#[test]
fn raw_commands_pass_through() {
    let mut client = Client::connect_simulated(&KIA_EV6).unwrap();
    let reply = client.raw_command("ATI").expect("ATI should answer");
    assert!(reply.contains("OK") || reply.contains("ELM"), "got: {reply:?}");
}

#[test]
fn polling_repeatedly_keeps_working() {
    let mut client = Client::connect_simulated(&KIA_EV6).unwrap();
    for i in 0..5 {
        let sample = client.poll();
        assert!(sample.soc().is_some(), "poll {i} lost SOC");
    }
}
