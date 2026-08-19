# BLE

The WiCAN Pro exposes a UART-style GATT service alongside Wi-Fi. Because
`Transport` is a trait, the BLE path reuses the ELM327 session and every decoder
unchanged — `ble.rs` only moves bytes.

```bash
./scripts/macos-ble-bundle.sh ble_scan     # discover + dump the GATT tree
./scripts/macos-ble-bundle.sh ble_probe    # full read of the car over BLE
```

## Does BLE survive sleep? No.

**This is the important part, and the answer is no.** The dongle does not "put
Wi-Fi to sleep" — it puts *itself* to sleep, and takes every radio down at once.
From `sleep_mode.c` in wican-fw:

```c
if (sleep_state == SLEEP_STATE) {
    can_disable();
    wifi_network_deinit();
    ble_disable();                          // BLE dies with Wi-Fi
    esp_sleep_enable_timer_wakeup(2*1000000);
    esp_light_sleep_start();
}
```

BLE is **not** a workaround for sleep. It is a workaround for *not having the
Wi-Fi network*, which is a different problem. Sleep thresholds, how to keep the
dongle reachable while parked, and the separate matter of the car's own ECUs
sleeping are all covered in [SLEEP.md](SLEEP.md).

## What BLE is actually good for

Station mode is convenient at home but useless away from that network. The
options away from home are access-point mode — which on Android hits the
[Wi-Fi routing problem](ANDROID.md#the-wi-fi-routing-gotcha) — or BLE, which has
no such issue because it never involves IP routing at all.

So: Wi-Fi at home, BLE on the road.

## GATT layout

Enumerated from the actual device with `ble_scan`:

```
service 0x180A                                    (device information)
  char 0x2A23 … 0x2A2A  READ

service 0xFFF0                                    <- data service
  char 0xFFF1  NOTIFY | INDICATE                  adapter -> us
  char 0xFFF2  WRITE | WRITE_WITHOUT_RESPONSE     us -> adapter
  char 0200dec0-01ef-bc9a-5678-1234deadf0be  NOTIFY | INDICATE
  char 0300dec0-01ef-bc9a-5678-1234deadf0be  WRITE | WRITE_WITHOUT_RESPONSE
```

Write an ELM327 command to `0xFFF1`'s sibling `0xFFF2`, read the reply as
notifications on `0xFFF1`. The framing is identical to the TCP socket, right
down to the `>` prompt, which is why the parser needed no changes.

The two 128-bit characteristics are a second channel this app does not use.

> **The firmware source is misleading here.** `main/ble.c` on the `main` branch
> declares service `0xFEE0` with characteristics `0xFEE1`/`0xFEE3`, marked
> `ESP_GATT_PERM_READ_ENC_MITM`. That is the **non-Pro** firmware. The Pro
> advertises `0xFFF0` and did not require pairing. Enumerate the device rather
> than trusting the repository.

## Pairing

The passkey was **not** required — `ble_probe` connected, subscribed and ran a
full poll with no pairing prompt, because the `0xFFF0` characteristics are not
declared encryption-protected on this firmware.

If a future firmware does demand pairing, the OS handles it: macOS and Android
raise their own passkey dialog. The app never sees or sends the passkey, so
there is nothing to store in the code. Keep it in `.env` (gitignored) or your
password manager.

## macOS: CoreBluetooth needs an app bundle

A plain CLI binary touching CoreBluetooth is killed by TCC with `Abort trap: 6`
and **no error output at all**. The crash report names the cause:

```
NSBluetoothAlwaysUsageDescription
```

Two things are needed, both handled by `scripts/macos-ble-bundle.sh`:

1. An `.app` bundle whose `Info.plist` declares
   `NSBluetoothAlwaysUsageDescription`, ad-hoc code signed so TCC's grant
   survives rebuilds.
2. Launching it with `open`, not by executing the inner binary. TCC attributes
   the request to the *responsible process*; run from a shell, that is the
   terminal, which cannot prompt. `open` makes the bundle itself responsible.

This applies to any macOS build of the dashboard that uses BLE, not just the
examples.

## Wiring BLE into the app

`ble_probe` shows the whole pattern:

```rust
let transport = BleTransport::connect(NAME_PREFIX, scan_for, timeout)?;
let mut client = Client::with_transport(Box::new(transport), &KIA_EV6)?;
let sample = client.poll();
```

The dashboard does not offer BLE yet — `worker.rs` builds a `TcpTransport`
directly. Adding it means extending `Command::Connect` with a transport choice
and putting a control in the UI; nothing below that layer changes.

BLE is behind the optional `ble` feature so the default build stays free of an
async runtime:

```bash
cargo build -p meatpi-core --features ble
```
