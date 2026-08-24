using System.IO.Ports;

namespace RustNet.Deploy;

/// <summary>Result of probing an Espressif chip's serial ROM bootloader.</summary>
public sealed record EspProbeResult(string ChipName, uint Magic, string? Mac, string BootLog);

/// <summary>
/// Minimal client for the Espressif serial ROM bootloader (the esptool
/// protocol): reset a board into download mode via the DTR/RTS circuit,
/// SYNC, read identification registers, reboot back to the app. This is
/// stage 0 of the hardware-in-the-loop rig — proving the tool ↔ silicon
/// serial link before RustNet firmware exists for the chip.
/// </summary>
public static class EspRom
{
    private const byte SlipEnd = 0xC0;
    private const byte SlipEsc = 0xDB;
    private const byte OpSync = 0x08;
    private const byte OpReadReg = 0x0A;

    /// <summary>Chip-detect magic register (same address on all Espressif chips).</summary>
    private const uint ChipMagicReg = 0x40001000;

    private static readonly Dictionary<uint, string> Magics = new()
    {
        [0x00F01D83] = "ESP32",
        [0x000007C6] = "ESP32-S2",
        [0x00000009] = "ESP32-S3",
        [0x6921506F] = "ESP32-C3",
        [0x1B31506F] = "ESP32-C3",
        [0x4881606F] = "ESP32-C3",
        [0x4361606F] = "ESP32-C3",
        [0x2CE0806F] = "ESP32-C6",
        [0xD7B73E80] = "ESP32-H2",
        [0x6F51306F] = "ESP32-C2",
    };

    /// <summary>
    /// Reset the board (EN pulse via RTS), capture the ROM boot banner for
    /// <paramref name="seconds"/>, and leave the app running.
    /// </summary>
    public static string CaptureBootLog(string port, int baud, double seconds)
    {
        using var sp = Open(port, baud);
        ResetToApp(sp);
        sp.DiscardInBuffer();
        return ReadFor(sp, seconds);
    }

    /// <summary>Enter download mode, identify the chip, reboot to the app.</summary>
    public static EspProbeResult Probe(string port, int baud)
    {
        using var sp = Open(port, baud);

        EnterBootloader(sp);
        string banner = ReadFor(sp, 0.4); // "waiting for download" etc.

        if (!Sync(sp))
        {
            throw new DeviceException(
                "chip did not answer the ROM SYNC — check wiring/drivers " +
                "(boot banner: " + Compact(banner) + ")");
        }

        uint magic = ReadReg(sp, ChipMagicReg);
        string chip = Magics.TryGetValue(magic, out var name) ? name : $"unknown (0x{magic:X8})";

        string? mac = null;
        if (chip == "ESP32")
        {
            // EFUSE BLK0 words 1..2 hold the base MAC (esptool layout).
            uint w1 = ReadReg(sp, 0x3FF5A004);
            uint w2 = ReadReg(sp, 0x3FF5A008);
            byte[] m =
            {
                (byte)(w2 >> 8), (byte)w2,
                (byte)(w1 >> 24), (byte)(w1 >> 16), (byte)(w1 >> 8), (byte)w1,
            };
            mac = string.Join(":", m.Select(b => b.ToString("x2")));
        }

        ResetToApp(sp);
        return new EspProbeResult(chip, magic, mac, Compact(banner));
    }

    // ------------------------------------------------------------------

    private static SerialPort Open(string port, int baud)
    {
        var sp = new SerialPort(port, baud, Parity.None, 8, StopBits.One)
        {
            ReadTimeout = 200,
            WriteTimeout = 1000,
            DtrEnable = false,
            RtsEnable = false,
        };
        sp.Open();
        return sp;
    }

    /// <summary>Classic esptool reset: EN low with IO0 held low → download mode.
    /// (DTR drives IO0, RTS drives EN, both inverted by the board circuit.)</summary>
    private static void EnterBootloader(SerialPort sp)
    {
        sp.DtrEnable = false;
        sp.RtsEnable = true;   // EN low (reset asserted)
        Thread.Sleep(100);
        sp.DtrEnable = true;   // IO0 low
        sp.RtsEnable = false;  // EN high (boot with IO0 low)
        Thread.Sleep(50);
        sp.DtrEnable = false;  // release IO0
        sp.DiscardInBuffer();
    }

    private static void ResetToApp(SerialPort sp)
    {
        sp.DtrEnable = false;  // IO0 released → normal boot
        sp.RtsEnable = true;   // EN low
        Thread.Sleep(100);
        sp.RtsEnable = false;  // EN high
    }

    private static bool Sync(SerialPort sp)
    {
        byte[] payload = new byte[36];
        payload[0] = 0x07;
        payload[1] = 0x07;
        payload[2] = 0x12;
        payload[3] = 0x20;
        for (int i = 4; i < 36; i++)
        {
            payload[i] = 0x55;
        }
        for (int attempt = 0; attempt < 7; attempt++)
        {
            SendCommand(sp, OpSync, payload);
            if (ReadResponse(sp, OpSync, out _))
            {
                // The ROM answers every queued sync; drain stragglers.
                Thread.Sleep(50);
                sp.DiscardInBuffer();
                return true;
            }
        }
        return false;
    }

    private static uint ReadReg(SerialPort sp, uint address)
    {
        SendCommand(sp, OpReadReg, BitConverter.GetBytes(address));
        if (!ReadResponse(sp, OpReadReg, out uint value))
        {
            throw new DeviceException($"READ_REG 0x{address:X8} got no response");
        }
        return value;
    }

    private static void SendCommand(SerialPort sp, byte op, byte[] data)
    {
        var frame = new List<byte> { 0x00, op };
        frame.AddRange(BitConverter.GetBytes((ushort)data.Length));
        frame.AddRange(BitConverter.GetBytes(0u)); // checksum (unused for these ops)
        frame.AddRange(data);

        var slip = new List<byte> { SlipEnd };
        foreach (byte b in frame)
        {
            if (b == SlipEnd)
            {
                slip.Add(SlipEsc);
                slip.Add(0xDC);
            }
            else if (b == SlipEsc)
            {
                slip.Add(SlipEsc);
                slip.Add(0xDD);
            }
            else
            {
                slip.Add(b);
            }
        }
        slip.Add(SlipEnd);
        sp.Write(slip.ToArray(), 0, slip.Count);
    }

    /// <summary>Read SLIP frames until a response to <paramref name="op"/>
    /// appears (or a short timeout elapses); yields the value word.</summary>
    private static bool ReadResponse(SerialPort sp, byte op, out uint value)
    {
        value = 0;
        var deadline = DateTime.UtcNow.AddMilliseconds(500);
        var frame = new List<byte>();
        bool inFrame = false;
        bool escaped = false;
        while (DateTime.UtcNow < deadline)
        {
            int b;
            try
            {
                b = sp.ReadByte();
            }
            catch (TimeoutException)
            {
                continue;
            }
            if (b < 0)
            {
                continue;
            }
            if (b == SlipEnd)
            {
                if (inFrame && frame.Count >= 8)
                {
                    if (frame[0] == 0x01 && frame[1] == op)
                    {
                        value = BitConverter.ToUInt32(frame.ToArray(), 4);
                        return true;
                    }
                }
                frame.Clear();
                inFrame = true;
                escaped = false;
                continue;
            }
            if (!inFrame)
            {
                continue; // boot noise outside frames
            }
            if (escaped)
            {
                frame.Add(b == 0xDC ? SlipEnd : b == 0xDD ? SlipEsc : (byte)b);
                escaped = false;
            }
            else if (b == SlipEsc)
            {
                escaped = true;
            }
            else
            {
                frame.Add((byte)b);
            }
        }
        return false;
    }

    private static string ReadFor(SerialPort sp, double seconds)
    {
        var deadline = DateTime.UtcNow.AddSeconds(seconds);
        var text = new System.Text.StringBuilder();
        var chunk = new byte[1024];
        while (DateTime.UtcNow < deadline)
        {
            try
            {
                int n = sp.Read(chunk, 0, chunk.Length);
                text.Append(System.Text.Encoding.ASCII.GetString(chunk, 0, n));
            }
            catch (TimeoutException)
            {
            }
        }
        return text.ToString();
    }

    private static string Compact(string s)
    {
        s = s.Replace("\r", "").Replace("\n", " | ");
        return s.Length > 160 ? s[..160] + "..." : s;
    }
}
