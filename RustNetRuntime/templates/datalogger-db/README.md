# __NAME__ — SQL data logger

Samples a DS18B20 temperature sensor over **1-Wire**, stores readings in
the on-device **SQL database** (`/data/logger.db`, flash-persisted — use
`/sd/...` for SD card or `/usb/...` for a USB drive), stamps rows with the
**RTC** and guards the loop with the **watchdog**. Prints a JSON summary
built with `RustNet.Serialization`.

```bash
dotnet build
rustnet flash bin/Debug/net10.0/__NAME__.dll --name logger --key <your.key> --start
rustnet logs -n 50
```

The virtual device ships a simulated DS18B20 at 25.5 °C on bus 0, so this
runs out of the box on `rustnet-firmware --ephemeral`.
