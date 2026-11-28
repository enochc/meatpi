# meathandler-server

Receives periodic telemetry from the WiCAN Pro in the car and keeps it on the
MeatHandler Raspberry Pi (10.43.1.57).

- `POST /api/wican` (HTTPS :8443, bearer token): what the dongle calls
- `GET /` (HTTP :8080): live dashboard
- `GET /api/latest`: newest value of every PID plus last-seen time
- `GET /api/history?key=SOC&hours=24`: time series for one PID
- `GET /api/device`: the dongle's boot-time status/config, secrets redacted

Data lives in SQLite at `/var/lib/meathandler/meathandler.db`.

## Build (Pi is 32-bit armv7, Raspbian trixie)

```bash
cross build --release --target armv7-unknown-linux-gnueabihf
cargo test
```

## Install / update

```bash
scp target/armv7-unknown-linux-gnueabihf/release/meathandler-server deploy/* enoch@MeatHandler.local:meathandler-install/
ssh -t enoch@MeatHandler.local 'cd ~/meathandler-install && sudo ./install.sh'
```

The first install creates a private CA and server cert in
`/etc/meathandler/certs` and a random token in `/etc/meathandler/env`.
Re-running keeps both, so the dongle config stays valid.

The server cert is issued for 10.43.1.57. Give the Pi a DHCP reservation, or
re-run `gen-certs.sh <new-ip> /etc/meathandler/certs` if it moves (the CA is
reused, so no new upload to the dongle is needed).

## WiCAN setup

The WiCAN only posts to `https://` URLs, so it must trust the local CA:

1. Upload `/etc/meathandler/certs/ca.crt` as a custom cert set.
2. Automate page: Grouping = Enable.
3. Add destination: type HTTPS, URL `https://10.43.1.57:8443/api/wican`,
   Auth = Bearer Token (value of `MH_TOKEN`), Cert Set = the custom set,
   cycle 10000 ms, Enabled. Submit and reboot the dongle.

## Certificates and login

The HTTPS port serves two certificates, picked by the name the client asks for:

- `meathandler.ddns.net`: the No-IP Vital Encrypt cert (`/etc/meathandler/certs/public.crt`),
  trusted by the WiCAN's default cert set and by browsers. Expires 2027-06-07.
  To renew, replace the PEM chain in `~/meathandler-ssl` (keep only one `.pem`
  there) and re-run `sudo ./install.sh`, which copies it in on every run.
- Anything else (e.g. `https://10.43.1.57:8443`): the local-CA cert.

The dashboard and read API use HTTP Basic auth (`MH_DASH_USER` / `MH_DASH_PASS`
in `/etc/meathandler/env`). Only port-forward 8443; keep 8080 on the LAN since
it is plain HTTP.

## 12 V battery alerts

Two sources, both sent through ntfy:

- **Regular posts:** if `LV_V` reads under `MH_LOW_12V` (default 12.2 V), one
  alert per low spell (re-arms at 12.6 V). Only works while the dongle is
  polling, which with Automate's minimum at 13 V means almost never while
  parked, so this is a backstop.
- **The WiCAN's own battery monitor:** while asleep it checks its supply
  voltage and, under its threshold, publishes over MQTT. `sudo
  ./setup-mqtt.sh` installs a LAN-only Mosquitto, sets `MH_MQTT_*`, and writes
  the WiCAN's Battery Alert settings to `~/meathandler-install/wican-mqtt.txt`.
  `src/battery_mqtt.rs` forwards those messages (at most every 6 h unless the
  voltage drops another 0.2 V).

## Web dashboard tab

The page at `/` has two tabs: **Values** (the plain tile list) and
**Dashboard**, which loads the meatpi Slint app compiled to WebAssembly from
`/app/` (served from `MH_WEB_DIR`, default `/usr/local/share/meathandler/web`,
gzipped, behind the same login). The browser build fetches `/api/latest`
same-origin every 5 s, so it uses the page's login and stores no password.

Build it from the meatpi repo root, then ship `web-dist/` as `web/` next to the
installer:

```bash
rustup target add wasm32-unknown-unknown
# wasm-bindgen CLI must match the wasm-bindgen crate version in Cargo.lock
WASM_BINDGEN=/path/to/wasm-bindgen bash meathandler-server/deploy/build-web.sh
scp meathandler-server/web-dist/* enoch@MeatHandler.local:meathandler-install/web/
```

The .wasm is about 11 MB (5 MB over the wire); browsers cache it after the
first load.

## Charge alerts

`src/charge_watch.rs` watches the `AC_PLUG` and `CHARGING` values. If the car
has been plugged in for `MH_PLUG_ALERT_SECS` (default 120) without charging,
it sends one push notification through [ntfy](https://ntfy.sh) to the topic in
`MH_NTFY_TOPIC`. With `MH_NOTIFY_CHARGE_START=1` it also announces each charge
start. Install the ntfy app and subscribe to the topic the installer prints.

It only sees what the dongle reports, so a dongle that stays asleep through the
plug-in produces no alert. A full battery or a charging schedule also reads as
"plugged in, not charging".

Logs: `journalctl -u meathandler -f`

## Home Assistant from outside

With `MH_HA_UPSTREAM` set (the installer defaults it to `10.43.1.77:80`),
the server also listens on `MH_HA_ADDR` (default `:8444`) and forwards that
port to Home Assistant over the LAN, using the same certificates as `:8443`.
The dashboard header has a **Home** link to it.

- Router: forward TCP 8444 to 10.43.1.57:8444, next to the existing 8443.
- Home Assistant does its own login on that port. Turn on two-factor
  authentication for every Home Assistant user.
- Home Assistant sees every remote client as 10.43.1.57, so no
  `trusted_proxies` entry is needed. Leave `ip_ban_enabled` off, or one bad
  login would ban the proxy and lock everyone out.
