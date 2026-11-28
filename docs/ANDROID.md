# Building for Android

## Prerequisites

- The `aarch64-linux-android` Rust target:
  ```bash
  rustup target add aarch64-linux-android
  ```
- An Android SDK with an NDK. On this machine the SDK is at
  `~/Library/Android/sdk`; `scripts/android-env.sh` finds the newest usable NDK
  under it.
- `cargo-apk`:
  ```bash
  cargo install cargo-apk
  ```

## Build and run

```bash
scripts/android-run.sh
```

It lists the connected devices by maker and model (the phone and the NXP board
both show up when the board is on ADB over TCP), asks which one to use, and
runs `cargo apk run -p meatpi-app --lib` against only that device. Pass a
serial from `adb devices` to skip the question. The underlying command, which
installs on whatever adb picks:

```bash
source scripts/android-env.sh
cargo apk run -p meatpi-app --lib
```

`--lib` matters: the Android entry point is `android_main` in the cdylib, not
the desktop binary. `src/main.rs` compiles to an empty `main` on Android so the
workspace still builds for that target.

To only type-check without packaging:

```bash
source scripts/android-env.sh
cargo check --target aarch64-linux-android -p meatpi-app
```

## How the Android build hangs together

- `crate-type = ["cdylib", "rlib"]` — the cdylib is what `NativeActivity` loads.
- `slint` gains the `backend-android-activity-06` feature only on Android, via a
  `[target.'cfg(target_os = "android")'.dependencies]` block.
- `android_main` in `lib.rs` initialises `android_logger`, calls
  `slint::android::init`, then the same `run()` the desktop uses.
- `[package.metadata.android]` in `Cargo.toml` declares the package id,
  permissions (`INTERNET`, `ACCESS_NETWORK_STATE`, `ACCESS_WIFI_STATE`) and
  `uses_cleartext_traffic = true` — the dongle serves plain TCP, which Android
  blocks by default since API 28.

The UI already adapts: below 820 px of width it stacks the gauge above a
scrollable tile grid instead of placing them side by side.

## The Wi-Fi routing gotcha

**This dongle is in station mode, so you probably dodge it** — it sits on the
normal Wi-Fi at `10.43.1.39`, the phone keeps its internet connection, and
sockets route normally. The rest of this section matters only if you switch the
dongle to access-point mode, which you would want for using the app away from
home.

When your phone joins the WiCAN Pro's access point,
Android notices the network has no internet access and keeps routing traffic
over cellular. Your socket to `192.168.0.10` then goes out the mobile interface
and times out, even though the Wi-Fi is connected and the dongle is right there.

Symptoms: works on desktop, times out on the phone; or works on the phone with
mobile data turned off but not with it on.

Two ways to deal with it:

1. **Quick check** — turn off mobile data, confirm the app connects. This proves
   the routing theory before you write any code.
2. **The real fix** — bind the process (or just the socket) to the Wi-Fi network
   using `ConnectivityManager.requestNetwork` with a
   `NetworkCapabilities.TRANSPORT_WIFI` request, then
   `Network.bindSocket`/`bindProcessToNetwork`. This needs a JNI call from Rust;
   `TcpTransport` in `meatpi-core` is the single place a bound socket would have
   to be injected, which is part of why the transport is behind a trait.

Option 2 is not implemented yet. If you plan to use the phone as the main
display, it is the first thing worth adding.

## Station mode vs access-point mode

Station mode (the current setup) sidesteps the routing problem entirely, but
only works within range of that network — fine on the driveway, useless on a
road trip. Access-point mode works anywhere and needs the socket binding above.

Supporting both means letting the user save two addresses; the app currently
just takes whatever you type into the address field.

## BLE

BLE avoids the routing problem entirely — no IP, no network for Android to
second-guess — which makes it the better answer for use away from home. It is
already implemented and verified on desktop (see [BLE.md](BLE.md)); service
`0xFFF0`, notify `0xFFF1`, write `0xFFF2`.

For Android you would additionally need:

- `BLUETOOTH_CONNECT` and `BLUETOOTH_SCAN` permissions (API 31+), requested at
  runtime, plus `ACCESS_FINE_LOCATION` on API 30 and below.
- btleplug's Android backend, which needs its Java companion class on the
  classpath — worth checking current btleplug Android support before committing
  to it, as it is less mature than the CoreBluetooth and BlueZ backends.

Note that BLE does **not** help when the dongle is asleep; the firmware disables
both radios together.
