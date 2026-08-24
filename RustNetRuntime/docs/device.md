# Writing device code

RustCLR runs the same C# on a desktop and on a microcontroller. What differs is
what is *underneath* it — and that is reached through one assembly,
`RustNet.Device`, whose namespaces mirror what a board actually has.

```bash
dotnet build managed/RustNet.Device -c Release
```

Reference it from a project, build, and run:

```bash
rustnet run --board host bin/Release/net10.0/App.dll
```

`--board host` installs a **simulator**: every peripheral against in-memory
state, so hardware code can be written and tested before a board is on the
desk. Without it there is no board at all, and a program that reaches for a pin
is told so.

---

## RustNet.Hal — the peripherals

```csharp
using RustNet.Hal;

Gpio.SetMode(2, PinMode.Output);
Gpio.Write(2, true);
Gpio.Toggle(2);

int millivolts = Adc.ReadMillivolts(0);
Pwm.Configure(channel: 0, frequencyHz: 1000, dutyPermyriad: 5000);  // 50%

byte[] reading = I2c.WriteRead(bus: 0, address: 0x48, command: new byte[] { 0x00 }, length: 2);

Uart.Configure(port: 1, baud: 115200);
Uart.Write(1, System.Text.Encoding.UTF8.GetBytes("hello\n"));
```

**Units are in the names.** `ReadMillivolts`, `dutyPermyriad`, `frequencyHz`,
`Micros`. An embedded API whose numbers are unlabelled is one where a factor of
a thousand is a runtime surprise; the parameter name makes the mistake visible
at the call site.

**`WriteRead` is one call for a reason.** Reading a sensor register is a write
followed by a read *without letting go of the bus*. Doing it as two calls lets
another thread's transfer land in the middle and gives you the wrong register.

**Nothing is clamped.** A pin mode outside the enum, a duty above 100%, a bus
with no device on it — each throws. A driver that got zeros back from a sensor
that is not there would report a plausible temperature, which is worse than an
error.

### Is there a board at all?

```csharp
if (Board.IsPresent)
{
    Console.WriteLine($"running on {Board.Name}");
}
```

These two answer anywhere. Everything else throws when there is no board,
naming what was missing.

---

## RustNet.IO — storage

```csharp
using RustNet.IO;

Storage.WriteAllText("/etc/hostname", "sensor-01");
Storage.AppendAllText("/var/log.txt", $"{DateTime.Now}: booted\n");

foreach (string name in Storage.List("/var"))
{
    Console.WriteLine(name);
}
```

Files are read and written **whole**. That is the shape of the medium, not a
subset of `System.IO`: NOR flash clears bits and cannot set them, so the only
way back is erasing a sector — and a `Stream` with `Seek` over that would mean
read-erase-write of the surrounding sector on every write, which is slow and
opens a power-loss window over data that was previously fine.

Writes append and the newest record wins; deletes append a tombstone. So the
store only grows until it is compacted, and `Storage.UsedBytes` is what a
program watches.

Directories are prefixes rather than objects. `CreateDirectory` writes an empty
marker so an empty one can still be listed.

---

## RustNet.Security — hashing, MACs, encryption

```csharp
using RustNet.Security;

string fingerprint = Hash.Sha256Hex(payload);

byte[] tag = Hmac.Compute(deviceKey, payload);
if (!Hmac.Verify(deviceKey, payload, receivedTag))
{
    return;   // constant-time: see below
}

byte[] sealed = Cipher.Transform(key, counterBlock, plaintext);
byte[] opened = Cipher.Transform(key, counterBlock, sealed);   // the same call
```

The algorithms are **written out** in `crates/rustclr-crypto` rather than
depended on, and checked against the vectors FIPS 180-4, FIPS 197,
NIST SP 800-38A and RFC 4231 publish.

Three things worth knowing before you use them:

- **Use `Hmac.Verify`, never `computed == expected`.** A normal comparison
  stops at the first differing byte, and how long it took says where that byte
  was — enough to forge a tag one byte at a time. `Verify` compares in constant
  time, and it exists as a method so that nobody has to remember this.
- **A (key, counter) pair must never be reused.** Counter mode XORs a keystream
  into the data; reusing a pair XORs two plaintexts together and both come back
  out. The runtime cannot check this — it does not know what you encrypted
  before — so it is your responsibility.
- **There is no RSA and no random number generator.** Signature verification
  needs bignum arithmetic, and a hand-rolled bignum with a side channel is
  worse than no check at all, because it is a check that reports success.
  Entropy belongs to the chip and comes through the HAL. `docs/limitations.md`
  has the full list.

---

## RustNet.Graphics — drawing

```csharp
using RustNet.Graphics;

Graphics.Open(240, 240);
int ink = Graphics.Rgb(0, 200, 120);
Graphics.Clear(Graphics.Rgb(0, 0, 0));
Graphics.FillRect(10, 10, 60, 20, ink);
Graphics.DrawText(12, 14, $"{celsius:F1} C", ink, 2);
Graphics.Present();
```

One canvas, RGB565, colours packed into an `int`. Drawing needs no board —
`Present` is the only call that does, which is what lets a UI be tested
headlessly. Reading a pixel outside the canvas gives `-1` rather than black,
because black is a colour a program might have drawn and "outside" is not.

The font is the runtime's own 8×8 table: ASCII, integer scaling, and a gap for
anything else. `MeasureText` says how wide a string will be before you draw it.

---

## Text

`System.Text.Encoding.UTF8` and `.ASCII`, and nothing else — every wire format
a device meets is one of the two, and .NET's remaining code-page tables are
larger than the whole heap on the boards this targets. ASCII substitutes `?`
for what it cannot encode and invalid UTF-8 decodes to U+FFFD, both matching
.NET exactly.

---

## What runs where

The simulator catches "this program calls a peripheral this board does not
have". It does not catch a race with a sensor's conversion time, and its
timings are not real. `docs/limitations.md` lists what else is outside the
lines — including that only the thread running `Main` carries the board, so a
`Task.Run` that touches a pin throws.

`tests/fixtures/Device` is the worked example: 64 checks over every binding
above, and every one of them fails if the binding underneath is wrong.
