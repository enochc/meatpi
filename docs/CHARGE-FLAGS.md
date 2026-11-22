# EV6 charge and plug flags

Found 2026-11-21 on the 2024 EV6 by reading raw UDS replies over the WiCAN in
ELM327 mode in three states (charging at 3.7 kW AC, plugged in but held by a
70% charge limit, unplugged), two reads each, and keeping only bits that were
stable within a state and differed between states. Raw replies are in
`captures/2026-11-21-charge-flags/`.

| Signal | ECU | PID | WiCAN expression | Charging | Plugged, idle | Unplugged |
|---|---|---|---|---|---|---|
| Charging | BMS `7E4` | `220101` | `B63:1` | 1 | 0 | 0 |
| AC plug in | VCMS `7E5` | `22E001` | `B12:0` | 1 | 1 | 0 |

The MeatPi "Kia: EV6" vehicle profile puts `CHARGING`, `AC_PLUG` and
`CHARGING_DC` at `220101 B15:7/5/6`. On this car that byte is `0x00` in every
state, so all three always read 0. The dongle's profile was edited to the
expressions above (backup of the original taken first). `CHARGING_DC` is still
the profile's guess; it needs a capture at a DC fast charger.

## WiCAN byte numbering

`Bn` counts every byte of the ISO-TP frames, including the PCI bytes, not just
the payload. With `d` = index into the data after the `62 xx xx` echo:

- the first frame holds `10 LL 62 xx xx d0`, so `d0` is `B5`
- each following frame adds one PCI byte, then 7 data bytes

Checked against known values: SOC `B10` = `d4`, pack current `S17,B18` =
`d10,d11`, pack voltage `B19:B20` = `d12,d13`. This is why the app's
`profile.rs` offsets and the WiCAN expressions differ by 6 or 7.

## Other candidates seen

Also changed with charging but not used: `220101 d5..d8` (power limits, 0 when
the HV system is off), several single-bit flags in `7E5 22E002`, and
`7E5 22E001 d13` (0 when unplugged, `0xA0` / `0xA5` when plugged, probably the
EVSE pilot duty or current limit).
