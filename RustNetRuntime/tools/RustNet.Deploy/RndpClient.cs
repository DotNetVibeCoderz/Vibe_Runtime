using System.Text;

namespace RustNet.Deploy;

public sealed class DeviceException(string message) : Exception(message);

/// <summary>Typed RNDP client used by the CLI, Workbench and VSCode bridge.</summary>
public sealed class RndpClient(IDeviceTransport transport) : IDisposable
{
    private readonly List<byte> _rx = new();

    /// <summary>Interval between the PING retries that absorb a boot.</summary>
    private const int ReadyRetryMs = 750;

    /// <summary>How long a live device gets to answer before we assume it is
    /// wedged and reset it. Two retries is enough for one that is merely busy
    /// in a long interpreter slice.</summary>
    private const int ProbeBeforeResetMs = 2000;

    public static RndpClient Connect(string? deviceSpec, int readyTimeoutMs = 20000)
    {
        var client = new RndpClient(TransportFactory.Open(deviceSpec));
        client.WaitUntilReady(readyTimeoutMs);
        return client;
    }

    /// <summary>
    /// Absorbs a reboot caused by opening the link, on transports that admit to
    /// causing one. Retries PING — the one command with no side effects — until
    /// the device answers, then discards the replies the extra pings earned.
    /// <para>
    /// Without this the first real request is written while the board is still
    /// in reset and is simply lost, so the call waits out its whole timeout
    /// against a device that came up seconds earlier and is listening. Retrying
    /// arbitrary commands instead would not do: re-sending a FLASH_APP chunk
    /// mid-stream corrupts the upload.
    /// </para>
    /// </summary>
    public void WaitUntilReady(int timeoutMs = 20000)
    {
        if (!transport.ResetsDeviceOnOpen || timeoutMs <= 0)
        {
            return;
        }
        // A device that is already talking must not be disturbed: `rustnet logs`
        // should never cost you the running app's state. So reset is a recovery
        // step, reached only after a short unanswered wait.
        if (PingWithin(Math.Min(timeoutMs, ProbeBeforeResetMs)))
        {
            return;
        }
        transport.ResetIntoApplication();
        _rx.Clear();
        if (PingWithin(timeoutMs))
        {
            return;
        }
        throw new DeviceException(
            "device did not answer PING, before or after a reset — check the port, "
            + "the baud rate, and that the firmware is running");
    }

    /// <summary>Ping until answered or the budget runs out.</summary>
    private bool PingWithin(int timeoutMs)
    {
        byte[] ping = new RndpFrame(Cmd.Ping, []).Encode();
        byte[] chunk = new byte[4096];
        var deadline = DateTime.UtcNow.AddMilliseconds(timeoutMs);
        while (DateTime.UtcNow < deadline)
        {
            transport.Write(ping);
            var next = DateTime.UtcNow.AddMilliseconds(ReadyRetryMs);
            while (DateTime.UtcNow < next)
            {
                int n = transport.Read(chunk, 200);
                if (n > 0)
                {
                    _rx.AddRange(chunk.AsSpan(0, n).ToArray());
                }
                if (TryTakeFrame() is not null)
                {
                    DiscardLateReplies();
                    return true;
                }
            }
        }
        return false;
    }

    /// <summary>
    /// Drop the replies still in flight from the readiness pings. RNDP replies
    /// carry a status byte, not the command they answer, so a leftover PING
    /// reply would be read as the answer to the next request.
    /// </summary>
    private void DiscardLateReplies()
    {
        byte[] chunk = new byte[4096];
        while (transport.Read(chunk, 300) > 0)
        {
            // Keep reading until the line has been quiet for the whole window.
        }
        _rx.Clear();
    }

    public RndpFrame Call(byte cmd, byte[] payload, int timeoutMs = 15000)
    {
        transport.Write(new RndpFrame(cmd, payload).Encode());
        byte[] chunk = new byte[65536];
        var deadline = DateTime.UtcNow.AddMilliseconds(timeoutMs);
        while (DateTime.UtcNow < deadline)
        {
            var frame = TryTakeFrame();
            if (frame is not null)
            {
                return frame;
            }
            int n = transport.Read(chunk, 500);
            if (n > 0)
            {
                _rx.AddRange(chunk.AsSpan(0, n).ToArray());
            }
        }
        throw new DeviceException("device did not answer (timeout)");
    }

    /// <summary>
    /// Pull the next whole frame out of the receive buffer, or null if one is
    /// not there yet.
    /// <para>
    /// A failed CRC here means "those two bytes were not a frame start", not
    /// "the protocol is broken": on a serial device the frames share the wire
    /// with log text, and the line glitches while a board is held in reset, so
    /// a stray 0x52 0x4E pair is expected rather than exceptional. Resync past
    /// it and keep looking — the caller's timeout is what reports real silence.
    /// </para>
    /// </summary>
    private RndpFrame? TryTakeFrame()
    {
        while (true)
        {
            SkipToMagic();
            if (_rx.Count < 9)
            {
                return null;
            }
            int consumed;
            RndpFrame? frame;
            try
            {
                consumed = RndpFrame.TryDecode(_rx.ToArray(), out frame);
            }
            catch (InvalidDataException)
            {
                _rx.RemoveAt(0);
                continue;
            }
            if (frame is null)
            {
                return null;
            }
            _rx.RemoveRange(0, consumed);
            return frame;
        }
    }

    /// <summary>
    /// Serial devices emit boot banners and log noise between frames —
    /// drop everything up to the next RNDP magic ("RN") so the decoder
    /// only ever sees frame starts.
    /// </summary>
    private void SkipToMagic()
    {
        int i = 0;
        while (i < _rx.Count && (_rx[i] != 0x52 || (i + 1 < _rx.Count && _rx[i + 1] != 0x4E)))
        {
            i++;
        }
        if (i > 0)
        {
            _rx.RemoveRange(0, i);
        }
    }

    private byte[] Expect(byte cmd, byte[] payload, int timeoutMs = 15000)
    {
        var frame = Call(cmd, payload, timeoutMs);
        if (!frame.IsOk)
        {
            throw new DeviceException(frame.PayloadText);
        }
        return frame.Payload;
    }

    public int Ping() => Expect(Cmd.Ping, Array.Empty<byte>())[0];

    public string Info() => Encoding.UTF8.GetString(Expect(Cmd.Info, Array.Empty<byte>()));

    public void ProvisionKey(byte[] publicKeyDer) => Expect(Cmd.ProvisionKey, publicKeyDer);

    public string ListApps() => Encoding.UTF8.GetString(Expect(Cmd.ListApps, Array.Empty<byte>()));

    public void FlashApp(string name, byte[] sealedImage)
    {
        byte[] nameBytes = Encoding.UTF8.GetBytes(name);
        byte[] payload = new byte[1 + nameBytes.Length + sealedImage.Length];
        payload[0] = (byte)nameBytes.Length;
        nameBytes.CopyTo(payload, 1);
        sealedImage.CopyTo(payload, 1 + nameBytes.Length);
        Expect(Cmd.FlashApp, payload, 60000);
    }

    public void EraseApp(string name) => Expect(Cmd.EraseApp, Encoding.UTF8.GetBytes(name));

    public void StartApp(string name) => Expect(Cmd.StartApp, Encoding.UTF8.GetBytes(name));

    public void StopApp() => Expect(Cmd.StopApp, Array.Empty<byte>());

    /// <summary>Set the app that auto-runs on power-up/reboot, or pass null/empty
    /// to disable autostart.</summary>
    public void SetAutostart(string? name) =>
        Expect(Cmd.SetAutostart, string.IsNullOrEmpty(name) ? Array.Empty<byte>() : Encoding.UTF8.GetBytes(name));

    public void FlashData(string remotePath, byte[] data)
    {
        byte[] pathBytes = Encoding.UTF8.GetBytes(remotePath);
        byte[] payload = new byte[2 + pathBytes.Length + data.Length];
        BitConverter.TryWriteBytes(payload.AsSpan(0, 2), (ushort)pathBytes.Length);
        pathBytes.CopyTo(payload, 2);
        data.CopyTo(payload, 2 + pathBytes.Length);
        Expect(Cmd.FlashData, payload, 60000);
    }

    public byte[] ReadData(string remotePath) => Expect(Cmd.ReadData, Encoding.UTF8.GetBytes(remotePath));

    public void SetConfig(string key, string value)
        => Expect(Cmd.SetConfig, Encoding.UTF8.GetBytes(key + "\n" + value));

    public string GetConfig(string key)
        => Encoding.UTF8.GetString(Expect(Cmd.GetConfig, Encoding.UTF8.GetBytes(key)));

    public void ConfigureWifi(string ssid, string psk)
        => Expect(Cmd.WifiConfig, Encoding.UTF8.GetBytes(ssid + "\n" + psk));

    public string GetLogs(int max = 100)
        => Encoding.UTF8.GetString(Expect(Cmd.GetLogs, BitConverter.GetBytes((uint)max)));

    public string GetPerf() => Encoding.UTF8.GetString(Expect(Cmd.GetPerf, Array.Empty<byte>()));

    /// <summary>JSON snapshot of simulated I/O (pins, buses, netifs) for the simulator panel.</summary>
    public string IoState() => Encoding.UTF8.GetString(Expect(Cmd.IoState, Array.Empty<byte>()));

    public void SetBootImage(ushort width, ushort height, byte[] rgb565)
    {
        byte[] payload = new byte[4 + rgb565.Length];
        BitConverter.TryWriteBytes(payload.AsSpan(0, 2), width);
        BitConverter.TryWriteBytes(payload.AsSpan(2, 2), height);
        rgb565.CopyTo(payload, 4);
        Expect(Cmd.SetBootImage, payload, 60000);
    }

    public byte[] GetBootImage() => Expect(Cmd.GetBootImage, Array.Empty<byte>());

    /// <summary>Returns (width, height, rgb565 little-endian pixel data).</summary>
    public (int Width, int Height, byte[] Pixels) GetDisplay()
    {
        byte[] data = Expect(Cmd.GetDisplay, Array.Empty<byte>());
        int w = BitConverter.ToUInt16(data, 0);
        int h = BitConverter.ToUInt16(data, 2);
        return (w, h, data[4..]);
    }

    public void OtaUpdate(byte[] sealedFirmware, Action<int, int>? progress = null)
    {
        Expect(Cmd.OtaBegin, Array.Empty<byte>());
        const int chunkSize = 4096;
        for (int off = 0; off < sealedFirmware.Length; off += chunkSize)
        {
            int len = Math.Min(chunkSize, sealedFirmware.Length - off);
            Expect(Cmd.OtaData, sealedFirmware.AsSpan(off, len).ToArray(), 60000);
            progress?.Invoke(off + len, sealedFirmware.Length);
        }
        Expect(Cmd.OtaEnd, Array.Empty<byte>(), 120000);
    }

    public string OtaConfirm() => Encoding.UTF8.GetString(Expect(Cmd.OtaConfirm, Array.Empty<byte>()));

    public string OtaRollback() => Encoding.UTF8.GetString(Expect(Cmd.OtaRollback, Array.Empty<byte>()));

    private static byte[] BpPayload(uint method, uint ilOffset)
    {
        byte[] payload = new byte[8];
        BitConverter.TryWriteBytes(payload.AsSpan(0, 4), method);
        BitConverter.TryWriteBytes(payload.AsSpan(4, 4), ilOffset);
        return payload;
    }

    public void DebugSetBreakpoint(uint method, uint ilOffset) =>
        Expect(Cmd.DebugSetBp, BpPayload(method, ilOffset));

    public void DebugClearBreakpoint(uint method, uint ilOffset) =>
        Expect(Cmd.DebugClearBp, BpPayload(method, ilOffset));

    public void DebugContinue() => Expect(Cmd.DebugContinue, Array.Empty<byte>());

    public void DebugStep() => Expect(Cmd.DebugStep, Array.Empty<byte>());

    public string DebugStack() => Encoding.UTF8.GetString(Expect(Cmd.DebugStack, Array.Empty<byte>()));

    public string DebugLocals() => Encoding.UTF8.GetString(Expect(Cmd.DebugLocals, Array.Empty<byte>()));

    /// <summary>Current debug state: null if the app is running, else the
    /// (method, ilOffset) it is paused at.</summary>
    public (uint Method, uint IlOffset)? DebugState()
    {
        byte[] r = Expect(Cmd.DebugState, Array.Empty<byte>());
        if (r.Length == 0 || r[0] == 0)
        {
            return null;
        }
        uint m = BitConverter.ToUInt32(r, 1);
        uint off = BitConverter.ToUInt32(r, 5);
        return (m, off);
    }

    public void Reboot() => Expect(Cmd.Reboot, Array.Empty<byte>());

    /// <summary>
    /// Reboot into the board's ROM bootloader rather than back into the image.
    /// </summary>
    /// <remarks>
    /// Only the RP2040 port answers this — it calls the boot ROM's
    /// <c>reset_to_usb_boot</c>, and the board comes back as the mass-storage
    /// device that accepts a UF2. A payload distinguishes it from a plain
    /// reboot, so a firmware that does not know about it resets as usual
    /// rather than failing.
    /// </remarks>
    public void RebootToBootloader() => Expect(Cmd.Reboot, [1]);

    public void Dispose() => transport.Dispose();
}
