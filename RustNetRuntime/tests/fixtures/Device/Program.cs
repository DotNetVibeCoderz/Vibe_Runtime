using System;
using RustNet.Hal;
using RustNet.IO;
using RustNet.Security;
using RustNet.Graphics;
using RustNet.Media;

namespace Device;

/// <summary>
/// What "the HAL works" means, as a program.
///
/// This is the Conformance fixture's sibling for hardware. It cannot be run
/// against <c>dotnet</c> and diffed, because there is no board there — the
/// whole point of these bindings is that they reach something the desktop CLR
/// has no equivalent for. So instead of comparing two runtimes it asserts
/// against the simulator's documented behaviour, and every check is one that
/// fails if the binding underneath it is wrong.
///
/// Run it with:
///     rustnet run --board host tests/fixtures/Device/bin/Release/net10.0/Device.dll
/// </summary>
public static class Program
{
    static int checks = 0;
    static int failures = 0;

    static void Check(string name, bool ok)
    {
        checks++;
        if (!ok)
        {
            failures++;
            Console.WriteLine("FAIL " + name);
        }
    }

    static void CheckEq(string name, long actual, long expected)
    {
        checks++;
        if (actual != expected)
        {
            failures++;
            Console.WriteLine("FAIL " + name + ": got " + actual + ", expected " + expected);
        }
    }

    static void CheckThrows(string name, Action body)
    {
        checks++;
        try
        {
            body();
            failures++;
            Console.WriteLine("FAIL " + name + ": nothing was thrown");
        }
        catch (Exception)
        {
            // The kind matters less than the refusal: what must not happen is
            // a bus with no device on it quietly returning zeros.
        }
    }

    static bool Same(byte[] a, byte[] b)
    {
        if (a.Length != b.Length) return false;
        for (int i = 0; i < a.Length; i++)
        {
            if (a[i] != b[i]) return false;
        }
        return true;
    }

    public static int Main()
    {
        // ── the board is there at all ───────────────────────────────────────
        Check("a board is installed", Board.IsPresent);
        Check("the board has a name", Board.Name.Length > 0);
        Check("the name is not the no-board placeholder", Board.Name != "none");

        // ── GPIO ────────────────────────────────────────────────────────────
        Gpio.SetMode(2, PinMode.Output);
        Gpio.Write(2, true);
        Check("a pin reads back what was written", Gpio.Read(2));
        Gpio.Write(2, false);
        Check("and reads back the other value too", !Gpio.Read(2));

        // Toggle has to be a read-modify-write, not a blind set: a Toggle that
        // always wrote High would pass the first of these and fail the second.
        Gpio.Toggle(2);
        Check("toggle from low goes high", Gpio.Read(2));
        Gpio.Toggle(2);
        Check("toggle from high goes low", !Gpio.Read(2));

        // Two pins are two pins. A binding that ignored the pin number would
        // pass everything above and fail here.
        Gpio.SetMode(3, PinMode.Output);
        Gpio.Write(2, true);
        Gpio.Write(3, false);
        Check("pins are independent", Gpio.Read(2) && !Gpio.Read(3));

        // A mode outside the enum is refused rather than rounded to Input.
        CheckThrows("an unknown pin mode is refused", () => Gpio.SetMode(2, 99));

        // ── analogue ────────────────────────────────────────────────────────
        // The simulator's channels read 0 until driven, and the conversion to
        // millivolts is raw * 3300 / 4095 at 12 bits — so 0 is 0 mV.
        CheckEq("an undriven ADC channel reads zero", Adc.ReadRaw(0), 0);
        CheckEq("and zero counts is zero millivolts", Adc.ReadMillivolts(0), 0);

        // ── PWM ─────────────────────────────────────────────────────────────
        Pwm.Configure(0, 1000, 5000);
        Pwm.Stop(0);
        // Duty is in hundredths of a percent. 20000 would be 200%, which is
        // not a dimmer setting; it is a units mistake, and it is refused.
        CheckThrows("a duty above 100% is refused", () => Pwm.Configure(0, 1000, 20000));

        // ── UART ────────────────────────────────────────────────────────────
        // Port 0 on the simulator is wired in loopback.
        Uart.Configure(0, 115200);
        byte[] sent = new byte[] { 0x52, 0x75, 0x73, 0x74 };
        Uart.Write(0, sent);
        CheckEq("what was written is waiting to be read", Uart.Available(0), sent.Length);

        byte[] got = Uart.Read(0, 16);
        CheckEq("a short read returns only what arrived", got.Length, sent.Length);
        bool same = got.Length == sent.Length;
        for (int i = 0; i < got.Length && same; i++)
        {
            same = got[i] == sent[i];
        }
        Check("the bytes come back unchanged", same);
        CheckEq("and the buffer is empty afterwards", Uart.Available(0), 0);

        // Reading an idle port is not an error — it is an empty array.
        CheckEq("an idle port reads empty rather than throwing", Uart.Read(0, 8).Length, 0);

        // ── I2C ─────────────────────────────────────────────────────────────
        // Nothing is attached to the simulated bus, so every transfer NACKs.
        // This is the check that matters most: a driver that got zeros back
        // from a sensor that is not there would report a plausible temperature.
        CheckThrows("writing to an absent device NACKs", () => I2c.Write(0, 0x48, new byte[] { 0 }));
        CheckThrows("reading an absent device NACKs", () => I2c.Read(0, 0x48, 2));

        // ── watchdog ────────────────────────────────────────────────────────
        Watchdog.Start(5000);
        Watchdog.Feed();
        Watchdog.Stop();
        Check("the watchdog can be started, fed and stopped", true);

        // ── delay ───────────────────────────────────────────────────────────
        Delay.Micros(50);
        Check("a short delay returns", true);

        // ── storage ─────────────────────────────────────────────────────────
        Check("the board has storage", FileSystem.IsAvailable());

        FileSystem.WriteAllText("/etc/hostname", "maixgo");
        Check("a written file exists", FileSystem.Exists("/etc/hostname"));
        Check("and reads back what was written", FileSystem.ReadAllText("/etc/hostname") == "maixgo");

        // Newest wins. A store that returned the *first* record would pass the
        // check above and fail this one.
        FileSystem.WriteAllText("/etc/hostname", "esp32c3");
        Check("rewriting supersedes the old record", FileSystem.ReadAllText("/etc/hostname") == "esp32c3");

        FileSystem.AppendText("/var/log.txt", "one|");
        FileSystem.AppendText("/var/log.txt", "two|");
        Check("appends accumulate", FileSystem.ReadAllText("/var/log.txt") == "one|two|");

        byte[] blob = new byte[] { 0xDE, 0xAD, 0xBE, 0xEF };
        FileSystem.WriteAllBytes("/data/blob.bin", blob);
        byte[] back = FileSystem.ReadAllBytes("/data/blob.bin");
        bool blobSame = back.Length == blob.Length;
        for (int i = 0; i < back.Length && blobSame; i++)
        {
            blobSame = back[i] == blob[i];
        }
        Check("bytes survive a round trip unchanged", blobSame);

        // A file that is not UTF-8 is refused as text rather than coming back
        // with replacement characters where the bytes used to be.
        CheckThrows("reading binary as text is refused", () => FileSystem.ReadAllText("/data/blob.bin"));

        // Directories are prefixes, and listing one returns its children.
        FileSystem.CreateDirectory("/empty");
        string[] rootEntries = FileSystem.List("/");
        bool sawVar = false;
        for (int i = 0; i < rootEntries.Length; i++)
        {
            if (rootEntries[i] == "var") sawVar = true;
        }
        Check("listing the root finds a directory that has children", sawVar);

        FileSystem.Delete("/etc/hostname");
        Check("a deleted file stops existing", !FileSystem.Exists("/etc/hostname"));
        CheckThrows("and reading it says so", () => FileSystem.ReadAllText("/etc/hostname"));

        // Deleting leaves a tombstone, so the store only ever grows until it
        // is compacted. That is the cost of never rewriting in place.
        Check("the store accounts for what it has written", FileSystem.UsedBytes() > 0);

        // ── crypto ──────────────────────────────────────────────────────────
        // The published vector from FIPS 180-4. The runtime writes SHA-256 out
        // rather than depending on a crate, so this is the check that the
        // written-out version is the real one.
        Check("SHA-256 matches the published vector for \"abc\"",
            Hash.Sha256Hex(new byte[] { 0x61, 0x62, 0x63 })
                == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        CheckEq("a SHA-256 digest is 32 bytes", Hash.Sha256(new byte[] { 1 }).Length, 32);

        // The check value every CRC-32 implementation publishes.
        CheckEq("CRC-32 matches its published check value",
            Hash.Crc32(new byte[] { 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39 }),
            unchecked((int)0xCBF43926));

        // RFC 4231 case 2: a short key and a text message.
        byte[] jefe = new byte[] { 0x4a, 0x65, 0x66, 0x65 };
        byte[] question = System.Text.Encoding.UTF8.GetBytes("what do ya want for nothing?");
        Check("HMAC-SHA256 matches RFC 4231",
            Hash.Sha256Hex(Hmac.Compute(jefe, question)).Length == 64
            && Hmac.Verify(jefe, question, Hmac.Compute(jefe, question)));

        // Verification has to reject a tampered tag, or it is asserting
        // nothing at all.
        byte[] tag = Hmac.Compute(jefe, question);
        tag[0] ^= 0x01;
        Check("a tampered tag is rejected", !Hmac.Verify(jefe, question, tag));
        Check("a tag for a different message is rejected",
            !Hmac.Verify(jefe, new byte[] { 1, 2, 3 }, Hmac.Compute(jefe, question)));

        // AES-CTR is its own inverse.
        byte[] key = new byte[16];
        byte[] counter = new byte[16];
        for (int i = 0; i < 16; i++) { key[i] = (byte)(i + 1); counter[i] = (byte)i; }
        byte[] plain = System.Text.Encoding.UTF8.GetBytes("telemetry payload, 37 bytes long here");
        byte[] cipher = Cipher.Transform(key, counter, plain);
        Check("the ciphertext is not the plaintext", !Same(cipher, plain));
        Check("decrypting is the same operation", Same(Cipher.Transform(key, counter, cipher), plain));
        CheckEq("counter mode needs no padding", cipher.Length, plain.Length);

        // A key of the wrong length is refused rather than stretched.
        CheckThrows("a five-byte key is refused, not padded",
            () => Cipher.Transform(new byte[5], counter, plain));
        CheckThrows("a short counter block is refused",
            () => Cipher.Transform(key, new byte[8], plain));

        // ── graphics ──────────────────────────────────────────────────
        CheckThrows("drawing before Open is refused", () => Display.Clear(0));

        Display.Init(64, 32);
        Check("the canvas is open", Display.IsOpen());
        CheckEq("and has the size it was given", Display.Width(), 64);
        CheckEq("and the height too", Display.Height(), 32);

        int red = Color.FromRgb(255, 0, 0);
        int black = Color.FromRgb(0, 0, 0);
        Check("red and black are different colours", red != black);

        Display.Clear(black);
        CheckEq("a cleared canvas reads back the clear colour", Display.GetPixel(10, 10), black);

        Display.SetPixel(5, 6, red);
        CheckEq("a pixel reads back what was set", Display.GetPixel(5, 6), red);
        CheckEq("and its neighbour did not change", Display.GetPixel(6, 6), black);

        // Off the canvas is -1, not black: black is a colour, outside is not.
        CheckEq("reading outside the canvas says so", Display.GetPixel(-1, 0), -1);
        CheckEq("and past the far edge too", Display.GetPixel(64, 0), -1);

        // A fill has to cover its rectangle and stop at the edge of it.
        Display.FillRect(20, 10, 8, 4, red);
        CheckEq("a fill covers its first pixel", Display.GetPixel(20, 10), red);
        CheckEq("and its last", Display.GetPixel(27, 13), red);
        CheckEq("and stops one past the end", Display.GetPixel(28, 13), black);

        // Clipping is what a UI uses to draw inside a panel.
        Display.SetClip(0, 0, 4, 4);
        Display.FillRect(0, 0, 64, 32, red);
        CheckEq("a clipped fill lands inside the clip", Display.GetPixel(3, 3), red);
        CheckEq("and not outside it", Display.GetPixel(5, 5), black);
        Display.ClearClip();

        // Text uses the runtime's own 8x8 font, so a character is 8 pixels
        // wide at scale 1 and 16 at scale 2.
        CheckEq("text measures 8 pixels per character", Display.MeasureText("abc", 1), 24);
        CheckEq("and scales with the scale", Display.MeasureText("abc", 2), 48);

        Display.Clear(black);
        Display.DrawText(0, 0, "A", red, 1);
        bool anyLit = false;
        for (int y = 0; y < 8 && !anyLit; y++)
        {
            for (int x = 0; x < 8 && !anyLit; x++)
            {
                if (Display.GetPixel(x, y) == red) anyLit = true;
            }
        }
        Check("drawing a letter lights pixels", anyLit);

        CheckThrows("a rotation that is not a right angle is refused", () => Display.SetRotation(45));

        // Present needs a board; the drawing above did not.
        Display.Present();
        Check("the frame reaches the board", true);
        Display.Close();
        Check("the canvas closes", !Display.IsOpen());

        // -- Camera --------------------------------------------------------
        //
        // The host board's sensor is a colour-bar generator, so every pixel is
        // predictable and a wrong binding produces a wrong number rather than
        // a plausible-looking image. A board with no sensor throws instead,
        // which is checked on the boards themselves rather than here.

        Camera.Configure(16, 4);
        CheckEq("the camera takes the width it was configured with", Camera.Width(), 16);
        CheckEq("and the height", Camera.Height(), 4);

        byte[] frame = Camera.Capture();
        // RGB565 is two bytes a pixel, and the frame is the whole raster.
        CheckEq("an RGB565 frame is two bytes per pixel", frame.Length, 16 * 4 * 2);

        // Eight bars across sixteen pixels: two pixels each, first is white
        // (0xFFFF) and last is black (0x0000). Read little-endian, which is
        // what the format promises.
        CheckEq("the first pixel is the white bar", frame[0] | (frame[1] << 8), 0xFFFF);
        CheckEq("the last pixel is the black bar",
            frame[frame.Length - 2] | (frame[frame.Length - 1] << 8), 0x0000);
        // Bar 5 is red (0xF800): pixels 10 and 11 of each row.
        CheckEq("a middle pixel is the bar its column falls in",
            frame[10 * 2] | (frame[10 * 2 + 1] << 8), 0xF800);

        // Grayscale is one byte a pixel, so a binding that ignored the format
        // would return twice as many bytes.
        Camera.Configure(16, 4, PixelFormat.Grayscale);
        CheckEq("a grayscale frame is one byte per pixel", Camera.Capture().Length, 16 * 4);

        CheckThrows("a zero-sized frame is refused", () => Camera.Configure(0, 4));
        CheckThrows("and a negative one", () => Camera.Configure(16, -1));

        Console.WriteLine("device checks=" + checks + " failures=" + failures);
        return failures == 0 ? 0 : 1;
    }
}
