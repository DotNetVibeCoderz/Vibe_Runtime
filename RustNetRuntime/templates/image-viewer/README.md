# __NAME__ — embedded image viewer

Bundles a GIF as an embedded resource and displays it, demonstrating
`RustNet.Resources` (assets travel inside the RNX) + `RustNet.Drawing`
(image decode) + `RustNet.Graphics` (blit). Replace `assets/logo.gif`
with your own BMP/GIF.

```bash
dotnet build
rustnet flash bin/Debug/net10.0/__NAME__.dll --name viewer --key <your.key> --start
rustnet display capture -o screen.ppm
```
