# RustNet App Templates

Instantiate with `rustnet new <template> <ProjectName>`.

| Template | Description |
|---|---|
| sensor-logger | Reads a TMP36 analog sensor and logs to the device filesystem |
| weather-check | Polls a weather HTTP endpoint over WiFi and shows the result |
| calculator | Interactive expression evaluator (arithmetic + precedence) |
| xox-game | Tic-tac-toe against a random AI on console + display |
| display-testing | Exercises the graphics API: shapes, text, animation |
| graphics-primitives | Low-level primitives showcase (nanoFramework-style): lines, filled circles/triangles/ellipses, rounded rects, gradients, clipping, matrix-rain — adapts to panel size, runs live on the M5Stack Tough |
| filesystem-test | Filesystem smoke test: write/read/append/list/delete |
| wifi-mqtt | Connects WiFi, publishes sensor telemetry over MQTT |
| mqtt-dashboard | Live MQTT session drawn on the device panel: telemetry out, command topic in, status lamp, counters and a scrolling inbox — reconnects on its own, runs on the M5Stack Tough |
| datalogger-db | 1-Wire DS18B20 → on-flash SQL database, RTC timestamps, watchdog, JSON summary |
| can-gateway | CAN frames bridged into Modbus holding registers; Ethernet up |
| ui-dashboard | XML-defined display UI (WPF/Glide-style) bound to live ADC readings |
| image-viewer | Embedded GIF via RustNet.Resources, decoded and shown on the display |
| cloud-telemetry | Azure IoT Hub (SAS) MQTT telemetry; AWS/GCP/IFTTT clients in RustNet.Cloud |

Every template expects the `RUSTNET_SDK` environment variable (or the
`RustNetSdk` MSBuild property) to point at the RustNet repo root so the
`RustNet.*` class libraries resolve.
