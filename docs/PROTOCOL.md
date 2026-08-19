# Protocol notes

## The link

The WiCAN Pro runs an ELM327-compatible interpreter (it also understands ELM329,
STN and VT instruction sets) on a raw TCP socket.

| | WiCAN | WiCAN **Pro** |
|---|---|---|
| Default AP address | `192.168.80.1` | `192.168.0.10` |
| TCP/UDP port | `3333` | `35000` |

Both are configurable from the dongle's web UI, along with the CAN protocol and
bitrate. If you put the dongle in station mode (joining your home Wi-Fi) it will
take a DHCP address instead — check your router or the dongle's status page.

MQTT is the other output mode the firmware offers. This app does not use it; if
you enable MQTT on the device, the ELM327 socket mode is disabled, so pick one.

## Session setup

`ElmSession::initialize` sends, in order:

| Command | Why |
|---|---|
| `ATZ` | reset the interpreter (slow — allow ~1 s) |
| `ATE0` | echo off, so replies are not prefixed with the request |
| `ATL0` | no extra linefeeds |
| `ATS1` | **spaces on** — lets the parser tell a 3-nibble CAN id from data bytes |
| `ATH0` | headers off |
| `ATCAF1` | CAN auto-formatting on: the adapter handles ISO-TP reassembly and flow control |
| `ATST32` | ~200 ms per-request timeout |

Then the vehicle profile's own init runs. For the EV6:

```
ATSP6         ISO 15765-4, 11-bit ids, 500 kbaud
ATFCSH7E4     flow-control header
ATFCSD300000  flow-control data
ATFCSM0       flow-control mode
```

This mirrors the init string MeatPi ships in their own Hyundai/Kia profiles.

## Parsing responses

ELM327 output shape varies with settings and firmware. Rather than assume one,
`elm.rs` normalises all of these:

```
7EC 03 41 0D 5A          headers on, single frame
0: 62 01 01 FF F7 E7     headers off, multi-frame with line index
7EC0:6201013EFFF7E7      packed, no spaces
```

The rules: drop everything before a `:` (line index plus any header printed with
it), then discard tokens 3 or 8 nibbles wide (11-bit and 29-bit CAN ids) and
decode the rest as bytes.

The decoded stream is then searched for the **positive response marker** — `62 01 01`
for request `220101`, `41 0D` for `010D` — and the payload is whatever follows.
Searching for the marker instead of assuming a fixed offset is what makes the
parser immune to whether the adapter printed headers or ISO-TP length bytes.

## Byte offsets

Offsets in `profile.rs` are indices into the payload *after* the marker. That
matches the `a, b, c, …` lettering used by the Torque/HKMC community PID lists,
where `a` = index 0 and `aa` = index 26.

Status column: ✅ confirmed against the car, ⚠️ plausible but unproven.

### `220101` — battery ECU `7E4`

| Field | Index | Scaling | Status |
|---|---|---|---|
| SOC (BMS) | 4 | ÷2 → % | ✅ 71 %, agrees with the dash SOC |
| Max regen | 5–6 | ÷100 → kW | ⚠️ reads 0 parked |
| Max discharge | 7–8 | ÷100 → kW | ⚠️ reads 0 parked |
| Pack current | 10–11 (signed) | ÷10 → A | ✅ |
| Pack voltage | 12–13 | ÷10 → V | ✅ 747.7 V |
| Battery max temp | 14 (signed) | °C | ✅ 35 °C |
| Battery min temp | 15 (signed) | °C | ✅ 33 °C |
| Module temps | 16–20 (signed) | °C | ✅ five sensors, 33–35 °C |
| Highest cell | 23 | ÷50 → V | ✅ 3.88 V |
| Lowest cell | 25 | ÷50 → V | ✅ 3.88 V |
| 12 V battery | 29 | ×0.1 → V | ✅ 14.0 V, matches `ATRV` |

The cell voltages are the strongest confirmation in the whole table:
747.7 V ÷ 3.88 V = **192.7**, and the 77.4 kWh EV6 pack has **192 cells** in
series. Two independently decoded fields agreeing on the pack topology means
both offsets are right.

> **Removed:** the Kona list puts *battery inlet temperature* at index 22. On
> this car index 22 decodes to **77 °C**, which is not a battery inlet
> temperature. E-GMP reports **five** module temperatures (16–20) where the
> Kona reports four, so everything past the module block shifts. Indices 23 and
> 25 land on the max/min cell voltages, which is what those slots actually are.
> The real inlet-temperature offset has not been identified.

### `220105` — battery ECU `7E4`

| Field | Index | Scaling | Status |
|---|---|---|---|
| Cell deviation | 20 | ÷50 → V | ✅ 0.00 V, consistent with max cell = min cell |
| Battery heater | 23 (signed) | °C | ⚠️ 32–35 °C, tracks pack temp — could be another sensor |
| State of health | 25–26 | ÷10 → % | ✅ exactly 100.0 % |
| SOC (dash) | 31 | ÷2 → % | ✅ 72 %, one point above the BMS value as expected |

### Not supported by this car

`010D` (vehicle speed) and `0146` (ambient temperature) both return `NO DATA`.
Like many EVs, the EV6 does not implement the legislated mode 01 PID set, so
these are not in the profile — leaving them in made every poll report failures.

## Truncated first responses

The first multi-frame request after protocol setup frequently comes back with
only frame 0:

```
03E
0: 62 01 01 EF FB E7       ← 6 of the promised 62 bytes
```

That leading `03E` is the adapter announcing the total length, which gives a
free integrity check: `declared_length()` parses it, and `query()` retries once
when fewer bytes arrive than promised. Subsequent requests return complete
responses, so one retry is enough.

Positive pack current means discharge; negative means regen or charging.
`Sample::power_kw()` derives power as `voltage × current ÷ 1000`, so it carries
the same sign convention.

## Re-verifying, or adapting to another car

The offsets started from the [JejuSoul/OBD-PIDs-for-HKMC-EVs](https://github.com/JejuSoul/OBD-PIDs-for-HKMC-EVs)
Kona/Niro EV list and were then checked against a 2024 EV6 — which is how the
index-22 error above was found. That repo has no E-GMP folder, so the same
caution applies to any car you point this at.

Use the probe, with the car awake:

```bash
cargo run -p meatpi-core --example probe -- 10.43.1.39:35000
```

It prints the adapter identity, the raw hex for every request, and the decoded
values, so you can re-index by hand against the tables above.

What to check, in order of how much it tells you:

1. **SOC against the car's own display** — must agree within a point or two.
2. **Pack voltage ÷ cell voltage** — should land on a whole number of cells
   (192 for the 77.4 kWh EV6). This catches errors in either field.
3. **12 V reading against `ATRV`** — the two should be within a few tenths.
4. **Temperatures** — a block of adjacent bytes all within a few degrees of each
   other is the module temperature array; its length tells you how far
   everything after it has shifted relative to the Kona layout.

Anything that decodes to a number you cannot explain physically (77 °C for a
battery inlet) is a wrong offset, not a surprising reading. All offsets live in
one `const` block in [`profile.rs`](../crates/meatpi-core/src/profile.rs), so
corrections are one line with no plumbing.

## Still open

- The real battery-inlet-temperature offset for E-GMP.
- Whether `220105` index 23 is genuinely the heater sensor.
- The available power/regen limits, which read 0 while parked. Re-run the probe
  while driving to confirm indices 5–6 and 7–8.
- Vehicle speed, which is not available over legislated OBD-II on this car and
  would need to be found on a manufacturer-specific PID.

## Adding a vehicle

Add a `VehicleProfile` const in `profile.rs` with its own init commands and
`PidRequest`s, and append it to `PROFILES`. Nothing else needs to change —
the UI reads labels and units from the signal table.

## Sources

- [MeatPi WiCAN firmware & docs](https://github.com/meatpiHQ/wican-fw)
- [WiCAN vehicle profile format](https://meatpihq.github.io/wican-fw/config/automate/new_vehicle_profiles/)
- [JejuSoul/OBD-PIDs-for-HKMC-EVs](https://github.com/JejuSoul/OBD-PIDs-for-HKMC-EVs)
