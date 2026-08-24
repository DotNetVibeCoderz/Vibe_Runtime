# graphics-primitives

A low-level graphics primitives showcase for RustNet's `RustNet.Graphics`
display API — a richer take on the nanoFramework *Graphics/Primitives* sample.

It cycles through scenes exercising every built-in drawing call and adds
app-side primitives the base library does not ship:

| Scene            | Demonstrates                                             |
|------------------|---------------------------------------------------------|
| Title            | `FillGradient`, color swatches, centered text at scale 4 |
| Lines & pixels   | `DrawLine` starburst (rainbow), `SetPixel` star field    |
| Rectangles       | `DrawRect`/`FillRect`, rounded rects, **clipping window** |
| Circles/ellipses | `DrawCircle`, filled circles, outlined/filled ellipses   |
| Triangles        | outlined + scanline-filled triangles                     |
| Gradients        | horizontal/vertical `FillGradient`, banded 2D blend      |
| Text             | `DrawText` at scales 1–4, colored labels                 |
| 3D cube          | rotating wireframe cube (integer fixed-point 3D + perspective) |
| Bouncing balls   | double-buffered animation (native `FillCircle`)          |
| Matrix rain      | falling-glyph finale with a bright head + fading trail   |

`Display.FillCircle` renders natively in the runtime, so animation stays smooth
without an interpreted per-pixel loop. On the M5 Tough the panel updates by DMA,
and animation scenes yield briefly each frame so the device stays responsive to
the deploy tools.

The filled circle, triangle, ellipse and rounded-rectangle helpers are pure
integer-math routines built on `SetPixel`/`DrawLine`/`FillRect`, so the demo
doubles as a reference for composing higher-level shapes from the primitives.

It adapts to whatever size `Display` reports (fills a 320×240 M5Stack Tough or
a 160×128 TFT alike) and prints a marker per scene to the console, so progress
is visible over `rustnet logs` even without eyes on the panel.

## Run

```bash
rustnet new graphics-primitives --name gfxdemo
rustnet flash gfxdemo/bin/Debug/net10.0/gfxdemo.dll \
    --name gfxdemo --chip esp32 --key keys/rustnet-signing.key --start \
    --device serial:COM5
rustnet logs --follow --device serial:COM5      # watch scene markers
```

On a device with a wired panel (e.g. the M5Stack Tough, built with the
firmware `board-m5tough` feature) the scenes render live. On the virtual
device, capture the framebuffer instead:

```bash
rustnet display capture -o demo.ppm
```

---

Made by **Gravicode Studios**, led by **Kang Fadhil**.
