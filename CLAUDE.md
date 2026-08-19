# CLAUDE.md

Slint + Rust dashboard reading live telemetry from a MeatPi WiCAN Pro plugged
into a 2024 Kia EV6. Desktop and Android share one codebase.

## Commands

```bash
cargo run                     # desktop, connects to a real dongle
MEATPI_SIM=1 cargo run        # desktop, built-in simulator — no car needed
MEATPI_CONNECT=1 cargo run    # connect on launch instead of clicking
cargo test                    # unit + simulator round-trip tests

# Raw frames + decoded values, no UI. Use this to check byte offsets.
cargo run -p meatpi-core --example probe -- 10.43.1.39:35000
cargo run -p meatpi-core --example probe -- --sim
```

The dongle lives at `10.43.1.39:35000` (station mode on the home network).

Android (see `docs/ANDROID.md`):

```bash
source scripts/android-env.sh
cargo apk run -p meatpi-app --lib     # --lib: entry point is android_main
cargo check --target aarch64-linux-android -p meatpi-app
```

## Architecture

Two crates, and the split is load-bearing:

- **`meatpi-core`** — protocol only. No Slint, no async runtime, no platform
  assumptions. Layers bottom-up: `transport` (byte pipe) → `elm` (ELM327 framing
  and hex reassembly) → `profile` (what to ask, how to decode) → `client`
  (connect + poll). Testable headless; shared verbatim by both targets.
- **`meatpi-app`** — Slint UI, desktop binary, Android entry point.

Keep protocol work in `meatpi-core`. If a change needs a Slint type to express
itself, it probably belongs in the app crate instead.

### Threading

The socket lives on a dedicated thread (`worker.rs`) and talks to the UI over a
channel. Two constraints follow, and violating either breaks the build or the app:

- The closure handed to `Worker::spawn` runs off the UI thread, so it must be
  `Send`. Capture only `slint::Weak` and the event; hop to the UI thread with
  `slint::invoke_from_event_loop` before touching any Slint object. This is why
  sample merging happens in the worker rather than the UI — `Rc<RefCell<…>>` in
  that closure will not compile.
- Never do a blocking socket read on the UI thread. Android kills apps that
  stall the main looper.

Shared UI state lives on the window itself (e.g. the log `VecModel`, retrieved
via `get_log_lines()` and downcast) rather than in a side-channel `Rc`.

## Slint notes

- `ui/theme.slint` holds the palette as a `global`; `ui/widgets.slint` has
  `StatTile`, `SocGauge`, `StatusPill`; `ui/app.slint` is the window.
- Tile values arrive from Rust **pre-formatted as strings**, so a PID that did
  not answer renders as `—` instead of a misleading `0`. Only the gauge takes a
  raw float, paired with a `soc-known` bool.
- The responsive branches (`if root.narrow`) sit inside a plain `Rectangle` with
  explicit `min-width`/`min-height`. Putting them straight into the outer
  `VerticalLayout` makes the window's layout info depend on its own width —
  Slint warns about the binding loop and may panic at runtime.
- Children of a plain `Rectangle` are positioned at `(0,0)`, not centred. Set
  `x`/`y` yourself.

## Protocol gotchas

- ELM327 output shape varies (headers on/off, multi-frame line indices, packed
  hex). `elm.rs` normalises all of them, then finds the **positive response
  marker** (`62 01 01` for `220101`) and takes the payload after it. Do not
  reintroduce fixed offsets from the start of the frame.
- Byte offsets in `profile.rs` index the payload *after* the marker, matching
  Torque's `a`=0, `aa`=26 lettering.
- **E-GMP is not the Kona.** The offsets started from the Kona/Niro community
  list; most carry over, but E-GMP reports five module temperatures where the
  Kona reports four, so the tail of `220101` is shifted. Anything past the
  temperature block must be re-verified, not assumed. `docs/PROTOCOL.md` marks
  each field confirmed or unproven — keep that table honest when you change
  offsets.
- The EV6 is an **800 V** pack (~747 V, 192 cells at ~3.88 V). Range checks
  written for 400 V Hyundai/Kia EVs will reject valid readings.
- The car answers `NO DATA` to legislated mode 01 PIDs. Don't re-add `010D`/`0146`.
- The first multi-frame request after setup often returns only frame 0. The
  adapter prints the total length first (`03E`), which `declared_length()` uses
  to detect the short read; `query()` retries once. Keep that check if you touch
  the parser.
- A PID failing is normal (the car sleeps); a poll where *every* request fails
  means the link died, and the worker drops the connection in that case.
- The dongle sleeps when the car is off and disappears from the network
  entirely — an `(incomplete)` ARP entry for its IP means "car is off", not a
  bug. Wake the car before debugging connection code. **Sleep kills BLE too**
  (`ble_disable()` sits next to `wifi_network_deinit()` in the firmware), so
  neither transport is a fallback for the other here.

## BLE (optional `ble` feature)

- Service `0xFFF0`, notify `0xFFF1`, write `0xFFF2` — enumerated from the real
  device. The firmware's `main/ble.c` says `0xFEE0/1/3`; that is the **non-Pro**
  firmware and does not apply. Trust `ble_scan` over the repo.
- No pairing was required; the passkey in `.env` went unused. Don't add
  passkey-handling code speculatively — if firmware ever demands it, the OS
  prompts and the app still never sees it.
- macOS kills any non-bundled process that touches CoreBluetooth with
  `Abort trap: 6` and **no output**. Use `scripts/macos-ble-bundle.sh`, which
  builds an `.app` with `NSBluetoothAlwaysUsageDescription` and launches it with
  `open` (TCC blames the responsible parent process, so running the inner binary
  from a shell still fails).
- `BleTransport` bridges async btleplug to the sync `Transport` trait with an
  owned multi-thread runtime plus an mpsc channel. Keep the runtime
  multi-threaded — the notification pump is a spawned task that must progress
  while `write_all`/`read` block on it.

## Conventions

- `meatpi-core` returns `Option`/`Result` rather than substituting defaults —
  "unknown" must stay distinguishable from "zero" all the way to the UI.
- Add a vehicle by adding a `VehicleProfile` const in `profile.rs` and appending
  it to `PROFILES`. No new plumbing should be needed.
- The app reads only. Nothing writes to the CAN bus.
