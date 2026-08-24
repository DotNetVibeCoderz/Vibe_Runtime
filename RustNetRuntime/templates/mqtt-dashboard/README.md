# __NAME__ — MQTT dashboard on the device panel

Live MQTT session rendered on the device's own display: telemetry out, commands in.

Connects to a broker, publishes telemetry on a timer, subscribes to a command
topic, and paints the whole session on the panel — status lamp, connection
details, counters, and a scrolling inbox of received messages. Everything is
also written to the console, so `rustnet logs` tells the same story when you
can't see the glass.

Built for the **M5Stack Tough** (320x240 ILI9342C), but the layout is derived
from whatever size `Display` reports, so a smaller panel still lays out
sensibly. The font is 8x8 and `DrawText` advances `8 * scale` per character, so
40 characters fit across 320 px at scale 1.

## Configure it

Edit the constants at the top of `Program.cs`:

| Constant | Meaning |
|---|---|
| `Ssid` / `Psk` | WiFi credentials — see the ESP32 note below |
| `Broker` | `host:port` (the runtime's MQTT client takes no scheme) |
| `ClientId` | MQTT client id; must be unique on the broker |
| `TopicTelemetry` | published to, once per loop |
| `TopicCommand` | subscribed to; drives the inbox |

The default broker is the public `broker.hivemq.com:1883`, so the demo works
without standing up your own. Point it at `mosquitto` on your LAN for a private
run. There is no TLS on this path — port 1883 is plaintext.

### WiFi on ESP32 targets is joined by the firmware, not by the app

This matters, because it is not what the code appears to say. On the ESP32
firmware the radio is brought up **at boot** from credentials held in device
config, and `Wifi.Connect(ssid, psk)` in managed code only records state for
`Wifi.IsConnected()` — it does not drive the radio. So set the credentials on
the device once:

```bash
rustnet wifi --ssid <ssid> --psk <password> --device serial:COMn
rustnet reboot --device serial:COMn
```

You do **not** need to edit `Ssid`/`Psk` for this to work. The dashboard's WIFI
and IP rows come from `Wifi.GetSsid()` and `Wifi.GetIp()`, which report what the
interface is actually associated with — verified on an M5Stack Tough showing the
real network and DHCP address while the constants were still the placeholders.
The constants only serve as the join request on boards where the HAL performs
the join, and as a fallback label if the interface reports nothing.

Keep real credentials out of source control — pass them to `rustnet wifi`, which
stores them in device config (encrypted), not in the app image.

## Drive it from a PC

Publish to the command topic and watch the panel react:

```bash
mosquitto_pub -h broker.hivemq.com -t rustnet/__NAME__/cmd -m "hello from my laptop"
mosquitto_pub -h broker.hivemq.com -t rustnet/__NAME__/cmd -m "ping"
mosquitto_pub -h broker.hivemq.com -t rustnet/__NAME__/cmd -m "clear"
mosquitto_sub -h broker.hivemq.com -t rustnet/__NAME__/telemetry   # see the telemetry
```

Two payloads are commands; anything else is just displayed:

- `ping` — replies with `{"pong":true}` on the telemetry topic
- `clear` — empties the inbox

## Why the dashboard ticks about once every 10 seconds

`Mqtt.Poll()` **blocks** until a message arrives or the client's 10-second read
timeout expires. The app therefore repaints *before* polling and lights a
`LISTENING` pill for exactly as long as it is parked in that call. A panel that
sits still for ten seconds at idle is the timeout, not a hang — the uptime
counter jumps in 10-second steps and then a fresh telemetry message goes out.
Inbound messages appear as soon as they arrive, without waiting for the timeout.

A dropped session is detected on **publish**, not on poll: a poll failure is
indistinguishable from an idle timeout across platforms (`WSAETIMEDOUT` on
Windows, `EAGAIN` on lwIP), whereas a write to a dead socket fails reliably.
When a publish fails the app marks the session dropped and reconnects.

## Build and flash

```bash
dotnet build
rustnet flash bin/Debug/net10.0/__NAME__.dll --name __NAME__ \
  --key <priv.der> --start --device serial:COMn
rustnet logs --follow --device serial:COMn
```

On the M5Stack Tough the firmware must be the panel-enabled build
(`--features board-m5tough`) flashed with the custom partition table — see
`docs/deploy-m5tough.md`. Against the virtual device (`rustnet-firmware`) the
same app runs headless and the panel is readable with
`rustnet display capture`.
