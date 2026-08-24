# __NAME__

IoT sensor logger: reads a TMP36 analog temperature sensor and appends CSV rows to the device filesystem.

```
dotnet build
rustnet flash bin/Debug/net10.0/__NAME__.dll --name __NAME__ --key <priv.der> --start
rustnet logs --follow
rustnet data pull temperature.csv
```
