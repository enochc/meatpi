# meatpi

A Rust + [Slint](https://slint.dev) dashboard that reads live data from a
**MeatPi WiCAN Pro** plugged into a **2024 Kia EV6**. One codebase, two targets:
a desktop app (macOS/Linux/Windows) and an Android app.

<!-- Screenshot: run `MEATPI_SIM=1 cargo run` to see this yourself. -->

Real output from the car:

```
┌─────────────────────────────────────────────────────────┐
│ MeatPi Dashboard               ● Live · 10.43.1.39:35000│
│ Kia EV6 (2024)                                          │
├──────────────────────┬──────────────────────────────────┤
│                      │  Pack voltage    Pack current    │
│         72.0         │  747.3 V         2.3 A           │
│    state of charge   │  Power           State of health │
│        1.7 kW        │  1.7 kW          100.0 %         │
│                      │  Battery min/max Cell low/high   │
│                      │  33 / 35 °C      3.88 / 3.88 V   │
└──────────────────────┴──────────────────────────────────┘
```

## Quick start

No car required — there is a built-in simulator:

```bash
MEATPI_SIM=1 cargo run
```

Against the car, through MeatHandler (the default):

```bash
MEATHANDLER_PASS=... MEATPI_CONNECT=1 cargo run
```

The dongle runs in **AutoPID** mode and pushes its readings to the MeatHandler
server (`meathandler-server/`), and the app reads them from
`https://meathandler.ddns.net:8443/api/latest`. That works at home or away.
Use the MeatHandler login (`MH_DASH_USER` / `MH_DASH_PASS` on the Pi), and use
the hostname rather than the Pi's IP, since only the hostname has the public
certificate. Without a terminal, type the password into the app.

After a successful connect the app saves the login (untick **Remember login**
to skip this) and connects on its own at the next launch. It lives in the
macOS Keychain / Windows Credential Manager / Linux Secret Service, or on
Android is encrypted with an AES-GCM key held in the Android Keystore with only
the ciphertext in app-private storage (`src/secrets/`). **Forget login**
removes it, and a login the server rejects is dropped automatically.

Talking to the dongle directly (untick **MeatHandler**, or `MEATPI_DIRECT=1`)
needs it switched back to **ELM327** protocol mode, at `10.43.1.39:35000` in
station mode or `192.168.0.10` in access-point mode. The console pane only
works in this mode.

Useful env vars: `MEATHANDLER_URL` / `MEATHANDLER_USER` / `MEATHANDLER_PASS`
fill in the login, `MEATPI_HOST` overrides the dongle address,
`MEATPI_CONNECT=1` connects on launch, `MEATPI_SIM=1` does both against the
simulator.

To check the MeatHandler path without the UI:

```bash
MEATHANDLER_PASS=... cargo run -p meatpi-core --features remote --example remote_probe
```

To dump raw frames and decoded values without the UI — this is the tool for
checking byte offsets:

```bash
cargo run -p meatpi-core --example probe -- 10.43.1.39:35000
```

## How it connects

The WiCAN Pro exposes a **TCP socket speaking ELM327** — the same AT-command
dialect as a classic OBD-II dongle. The app opens that socket, configures the
interpreter, and polls a handful of PIDs on a timer:

```
app ──TCP──▶ WiCAN Pro ──CAN──▶ EV6 battery ECU (7E4)
   "220101\r"                     UDS request
   "62 01 01 FF F7 …"             response, ISO-TP reassembled by the adapter
```

Two request families are in use:

| Request  | ECU   | What it carries                                             |
|----------|-------|-------------------------------------------------------------|
| `220101` | `7E4` | SOC, pack voltage/current, module temps, cell high/low, 12 V |
| `220105` | `7E4` | State of health, dashboard SOC, cell deviation               |

The legislated OBD-II PIDs (`010D` speed, `0146` ambient) return `NO DATA` on
this car, as on many EVs, so they are not polled.

See [docs/PROTOCOL.md](docs/PROTOCOL.md) for the byte-level decoding, which
fields are confirmed against the car, and how to re-verify.

## Layout

```
crates/
  meatpi-core/     no UI, no async runtime — pure protocol + decoding
    transport.rs   TCP socket (and the trait the simulator implements)
    elm.rs         ELM327 framing, hex reassembly, response marker search
    profile.rs     which PIDs to ask for, and how to decode each field
    telemetry.rs   a Sample: decoded values keyed by signal name
    client.rs      connect + poll
    sim.rs         a fake WiCAN Pro for development
  meatpi-app/      Slint UI, desktop binary, and the Android entry point
    ui/*.slint     theme, widgets, dashboard
    worker.rs      the connection thread
    lib.rs         UI wiring; `android_main` lives here too
```

`meatpi-core` knows nothing about Slint, so the protocol layer is unit-testable
without a display and is shared verbatim by both targets.

## Building for Android

Short version:

```bash
source scripts/android-env.sh
cargo install cargo-apk        # once
cargo apk run -p meatpi-app --lib
```

Full instructions, including the Wi-Fi routing gotcha that will otherwise make
the dongle unreachable from your phone, are in [docs/ANDROID.md](docs/ANDROID.md).

## Tests

```bash
cargo test
```

The suite covers the ELM327 output shapes (headers on/off, multi-frame,
whitespace-free), the decoders, and a full round trip through the simulator.

## Status

This is an initial scaffold. It reads; it does not write to the bus.

Verified live against the car on 18 Aug 2026 — SOC, pack voltage and current,
module temperatures, cell high/low, state of health and the 12 V reading all
decode correctly. Two fields cross-check each other: 747.7 V ÷ 3.88 V per cell
= 192.7, and the 77.4 kWh EV6 pack has 192 cells in series.

Still open:

- **Available power/regen limits read 0 while parked.** Defensible, but unproven
  — re-run the probe while driving.
- **The battery inlet temperature offset is unknown.** The Kona list puts it at
  index 22; on E-GMP that decodes to 77 °C, so it was removed rather than shown
  wrong. E-GMP has an extra module temperature sensor that shifts the tail of
  the frame.
- **Vehicle speed is not available** over legislated OBD-II here; it would need a
  manufacturer-specific PID.
- **BLE works but is not in the UI yet.** `BleTransport` reads the car
  end-to-end (`ble_probe`), reusing the ELM327 and decoding layers unchanged.
  The dashboard still builds a TCP transport directly; exposing the choice means
  a control in the UI and one more field on `Command::Connect`.
- Only one vehicle profile exists. Adding another is a `const` in `profile.rs`,
  not new plumbing.

## Why this app only reads

The WiCAN transmits CAN fine — the hardware was never the limitation. The
limitation is **which bus the OBD-II port reaches**.

Climate and remote lock live on **M-CAN**, the multimedia bus. It is not on the
OBD connector's pins, so nothing plugged into that port can send those commands,
no matter what firmware it runs.

This is a solved problem, just not from the OBD port:
[tylerharvey/Ioniq5_CAN](https://github.com/tylerharvey/Ioniq5_CAN) reverse
engineered E-GMP preconditioning **using a WiCAN Pro**, and ships kits running
stock WiCAN hardware — with a harness that taps M-CAN at the head-unit connector
and routes it to a female OBD socket. Installing it means pulling dash trim.

Full analysis, including a diff of the published climate captures and the
practical alternatives, is in [docs/REMOTE-CONTROL.md](docs/REMOTE-CONTROL.md).

Sending frames into a live vehicle bus is not free — it can set DTCs and confuse
modules mid-drive — which is why this app reads only, and why adding transmit
would be gated behind being on the right bus in the first place.

## BLE

The dongle also speaks ELM327 over a BLE GATT service, which is useful away from
your Wi-Fi. See [docs/BLE.md](docs/BLE.md).

```bash
./scripts/macos-ble-bundle.sh ble_probe    # read the car over Bluetooth
```

**BLE does not survive sleep.** The firmware calls `ble_disable()` in the same
breath as `wifi_network_deinit()`, so a sleeping dongle is unreachable by both
transports — turning the car on is the only fix. BLE solves *"I'm not on that
Wi-Fi network"*, not *"the dongle is asleep"*.

On macOS, anything using BLE must run from an app bundle declaring
`NSBluetoothAlwaysUsageDescription`, launched via `open` — a bare binary is
killed by TCC with no error message. `scripts/macos-ble-bundle.sh` handles both.
