# Remote HVAC and start: what's actually possible

Short answer: **there is no OBD-port workaround.** The commands exist and are
reachable with WiCAN hardware, but not from the port the dongle plugs into. Wire
work inside the dash is unavoidable.

## Why the OBD port can't do it

The OBD-II connector reaches the diagnostic bus. Climate and remote lock live on
**M-CAN** (the multimedia bus), which is not present on the OBD connector's pins
and is not reachable through the gateway with arbitrary frames.

## The project that solved this

[tylerharvey/Ioniq5_CAN](https://github.com/tylerharvey/Ioniq5_CAN) — an active,
open-source effort covering Ioniq 5/6 and EV6, with an
[EV6 install guide](https://github.com/tylerharvey/Ioniq5_CAN/blob/main/guides/cars/E-GMP_gen1/EV6/head_unit_preconditioning_kit_EV6_install.md).

Two details make it directly relevant here:

- They reverse-engineered the messages **using a WiCAN Pro** with SavvyCAN — the
  same hardware in this car.
- The shipping kit runs **stock WiCAN-OBD-C3 units** with custom firmware
  ([wicant-i-precondition](https://github.com/L1Z3/wicant-i-precondition)).

So the hardware was never the obstacle. The obstacle is *where it plugs in*.

Their harness taps M-CAN at the head-unit connector and routes it out to a
female OBD socket, so a normal OBD dongle can be plugged into M-CAN instead of
the diagnostic bus. Installing it means removing crash-pad trim and
disconnecting the 12 V battery first.

### Important scope limit

The shipping product does **battery** preconditioning — warming the traction
pack before DC fast charging. That is not cabin HVAC. Do not assume buying the
kit gets you remote climate.

## Cabin climate frames: a lead, not an answer

The repo publishes raw M-CAN captures, including a control log and one taken
while remote climate was started. Diffing message IDs between them:

| ID | control | climate start | climate stop | remote lock |
|---|---|---|---|---|
| `0xC7` | – | 12 frames | – | – |
| `0x4A2` | – | 12 frames | – | 36 frames |

`0xC7` appears **only** in the climate-start capture — a ~180 ms burst where the
payload goes `00 00 00 01 00 00 00 00` then back to `00 00 00 00 00 00 00 00`.
Byte 3 flipping to `01` and back reads like a momentary request flag.

`0x4A2` shows up in both climate-start and remote-lock, so it is more likely a
generic remote/telematics envelope than a climate command.

**The inconsistency worth noting:** the climate-*stop* capture contains neither
ID. If `0xC7` were simply "climate on/off", stop should carry it. Either that log
did not capture the command, or stop is signalled another way. Until that is
explained, `0xC7` is a hypothesis — not a verified command, and not something to
transmit at a car on the strength of this table.

Analysis reproduced from
[`CAN_logs/panda/`](https://github.com/tylerharvey/Ioniq5_CAN/tree/main/CAN_logs/panda).

## The pragmatic option: key fob relay

Given Kia Connect is unavailable, this is the best effort-to-result trade and
needs no dash teardown:

The EV6 key fob has remote start, which triggers climate. Pressing a fob button
just shorts two contacts, so an ESP32 or ESP8266 with a relay (or transistor)
soldered across a **spare** fob's contacts gives a remotely triggerable button.
Keep the spare fob in the car or garage and trigger it over Wi-Fi.

Why this is attractive:

- Uses the car's own authenticated RF channel — no frame injection, no bus risk,
  nothing to reverse-engineer.
- No subscription, unaffected by the registration problem.
- Well-trodden: see [Hackaday's writeup](https://hackaday.com/2021/01/06/automating-your-car-with-a-spare-fob-and-an-esp8266/)
  and the [Home Assistant thread](https://community.home-assistant.io/t/car-lock-unlock-remote-start-esp8266-and-a-spare-keyfob/589701).

Costs: a spare fob (the real expense — EV6 smart keys need dealer programming),
and fob range, which is why the fob lives near the car rather than with you.

Being a physical bridge to the car's key, treat where you leave the fob as a
security decision, not just a placement one.

## Fixing Kia Connect

Still worth pursuing in parallel — it is the only route that is supported,
works at any distance, and does not care whether the dongle or the car is
asleep. A vehicle still registered to a previous owner is a routine VIN-release
request Kia support can clear. Worth asking about a complimentary trial at the
same time.

Once available, [hyundai_kia_connect_api](https://github.com/Hyundai-Kia-Connect/hyundai_kia_connect_api)
gives remote start, climate and lock/unlock programmatically — a separate module
in this app, unrelated to the CAN layer.

## Where this leaves the app

Still read-only. Sending anything meaningful would require the app to be on
M-CAN, which is a wiring change, not a software one. If that harness ever gets
installed, `meatpi-core` needs a transmit path — the `Transport` trait already
carries bytes both ways, so it would be new API on `Client`, not a redesign.
