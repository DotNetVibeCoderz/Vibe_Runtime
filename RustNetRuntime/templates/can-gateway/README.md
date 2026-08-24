# __NAME__ — CAN ↔ Modbus gateway

Reads **CAN** frames (loopback demo; acceptance filter 0x100–0x1FF),
bridges them into **Modbus** holding registers via the on-device slave,
and brings up **Ethernet**. On real hardware, point the Modbus master
calls at your PLC's unit id and drop the CAN loopback flag.

```bash
dotnet build
rustnet flash bin/Debug/net10.0/__NAME__.dll --name gateway --key <your.key> --start
rustnet logs -n 50
```
