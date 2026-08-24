# __NAME__ — cloud telemetry

Publishes ADC sensor readings as JSON to **Azure IoT Hub** over MQTT with
SAS-token auth (the token is signed on-device with HMAC-SHA256). Swap in
`AwsIotCore`, `GoogleIotCore`, or `Ifttt` from `RustNet.Cloud` for other
providers.

Fill in `HubHost`, `DeviceId` and `DeviceKeyBase64` in `Program.cs`, then:

```bash
dotnet build
rustnet flash bin/Debug/net10.0/__NAME__.dll --name cloud --key <your.key> --start
rustnet logs -n 50
```

The real hub needs a TLS-capable build (ESP-IDF provides mbedTLS); the
virtual device exercises the token signing and payload construction and
attempts the (plaintext) connect. `RustNet.Cloud` also provides AWS IoT
Core (Device Shadow), Google Cloud IoT (JWT), and IFTTT Webhooks.
