# Reaching the car while it's parked

There are **two** independent things that go to sleep, and both must be awake
for a poll to return data.

## 1. The dongle

Controlled entirely by the WiCAN config (web UI). Firmware defaults:

| Setting | Default | Meaning |
|---|---|---|
| `sleep_status` | `disable` | whether the dongle sleeps at all |
| `sleep_volt` | `13.1` | sleep below this 12 V reading |
| `wakeup_volt` | `13.5` | wake above this |
| `sleep_time` | `2` | minutes below `sleep_volt` before sleeping (1–30) |

When it sleeps it calls `can_disable()`, `wifi_network_deinit()` and
`ble_disable()` together, then light-sleeps with a 2 s timer to re-sample
voltage. Waking is a full `esp_restart()`, so allow a few seconds to rejoin the
network. **No transport reaches a sleeping dongle** — Wi-Fi and BLE die
together.

The 13.1 V threshold sits between a running car (~14 V) and a resting 12 V
battery (~12.4 V), so with sleep enabled the dongle sleeps whenever the car is
off.

### Do not lower `sleep_volt` to stay reachable

An earlier version of this document suggested dropping `sleep_volt` to ~12.2 V
to keep the dongle awake while parked. **That is bad advice — do not do it.**

13.1 V is chosen precisely because it sits *above* a resting battery (~12.4–12.8 V)
and *below* a charging one (~14 V). Set it to 12.2 and a parked car never falls
below the threshold, so the dongle simply never sleeps. It only gives up once
the battery has sagged to 12.2 V, which is well into the discharge you were
trying to prevent. You get maximum drain and a cut-off that arrives too late.

Leave it at 13.1.

### The drain maths

Sleeping draws **under 1 mA** (MeatPi's figure). Awake with Wi-Fi is roughly
100 mA — my estimate; only the sleep number is published.

| Configuration | Daily draw | Over a 30-day trip |
|---|---|---|
| Sleep enabled, idle | ~0.024 Ah | ~0.7 Ah |
| Sleep + hourly periodic wake-up | ~0.045 Ah | ~1.3 Ah |
| Sleep disabled | ~2.4 Ah | ~72 Ah |

Against a ~45–60 Ah 12 V battery — which the car is also drawing from — the
first two are noise and the third flattens it in well under three weeks.

The useful conclusion: **periodic wake-up is cheap.** A wake, connect, publish
and sleep cycle costs well under a milliamp-hour. Even hourly it is a rounding
error. What kills the battery is never sleeping at all, not waking often.

### Recommended settings

| Setting | Value | Why |
|---|---|---|
| `sleep_status` | **enable** | ships disabled in some firmware — check yours |
| `sleep_volt` | `13.1` | the default is correct; see above |
| `wakeup_volt` | `13.5` | default; accepted range 12.0–15.0 |
| `sleep_time` | `2`–`3` min | how long below threshold before sleeping |
| `periodic_wakeup` | **enable** | wakes, joins Wi-Fi, sleeps again |
| `wakeup_interval` | `60`–`90` min | slider range is 5–240 |
| `batt_alert` | *unavailable* | see below |

### Battery alert is not available on the Pro

On **WiCAN-OBD-PRO firmware v4.50p** — the current release — the Battery Alert
control is served with a single option:

```html
<select id=batt_alert name=batt_alert>
  <option value=disable>Disable</option>
</select>
```

There is no "Enable". Nothing adds one dynamically, and the neighbouring
`periodic_wakeup` control has both options, so this is deliberate rather than a
rendering bug. The `batt_alert_*` keys still appear in exported configs, which
makes the feature look configurable when it is not — they are vestigial on this
build. The alert logic exists in the non-Pro firmware source.

Updating will not help: v4.50p is already the latest release. So the MQTT
broker and `batt_alert_ssid` settings are moot here — nothing publishes.

### Use the status endpoint instead

The dongle reports its own supply voltage over plain HTTP, no ELM327 session and
no CAN traffic:

```bash
curl -s http://wican_14c19fd9573d.local/check_status | python3 -c \
  "import sys,json; print(json.load(sys.stdin)['batt_voltage'])"
# 14.2V
```

Because that reading is the dongle's own ADC, **it works while the car's ECUs
are asleep** — which is exactly the property the battery alert would have given
us. Pair it with `periodic_wakeup` and the dongle comes up every interval long
enough to be scraped, then sleeps again.

Prefer the mDNS name `wican_<ble-mac>.local` over the IP: it survives DHCP
changes. (The app still defaults to the IP, since Android mDNS resolution needs
`NsdManager` rather than plain name lookup.)

That moves the alerting off the dongle and onto whatever does the scraping,
where the threshold is a one-line change rather than a firmware limitation.

## 2. The car

This is the part that surprises people. Even with the dongle wide awake, the
EV6's ECUs sleep when the car is off. The BMS on `7E4` simply stops answering,
and every request returns `NO DATA`.

The car is awake when it is on, while AC/DC charging, and in brief windows when
it runs the DC-DC converter to top up the 12 V battery from the traction pack.

This is why periodic wake-up gives you 12 V voltage but **not** state of charge
or anything else on `7E4`: voltage is the dongle's own ADC reading, while every
CAN value needs an ECU awake to answer. Expect a voltage trend line from a
parked car, not a telemetry feed.

So "can I connect while parked?" splits into two answers:

- **Reach the dongle while parked** — yes, by configuring sleep.
- **Get battery data while parked** — only during the windows above. Not
  continuously, no matter how you configure the dongle.

## Testing it on your car

Worth doing once, since the behaviour depends on your `sleep_status` setting and
on how aggressively your car's gateway sleeps:

1. With the car on, confirm `ping <dongle-ip>` responds.
2. Turn the car off and leave it.
3. After ~5 minutes: `ping` again, and `arp -n <dongle-ip>`.
   - `(incomplete)` → the dongle is asleep (or unpowered).
   - Responds → the dongle is awake; continue to step 4.
4. Run the probe. If it connects but every PID returns `NO DATA`, the dongle is
   awake and the *car* is asleep.

That tells you which of the two sleepers you are actually fighting.
