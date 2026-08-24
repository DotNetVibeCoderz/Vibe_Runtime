namespace RustNet.Deploy;

/// <summary>RNDP command codes (mirror of runtime/firmware/src/proto.rs).</summary>
public static class Cmd
{
    public const byte Ping = 0x01;
    public const byte Info = 0x02;
    public const byte ProvisionKey = 0x03;
    public const byte ListApps = 0x10;
    public const byte FlashApp = 0x11;
    public const byte EraseApp = 0x12;
    public const byte StartApp = 0x13;
    public const byte StopApp = 0x14;
    public const byte SetAutostart = 0x15;
    public const byte FlashData = 0x20;
    public const byte ReadData = 0x21;
    public const byte SetConfig = 0x30;
    public const byte GetConfig = 0x31;
    public const byte WifiConfig = 0x32;
    public const byte GetLogs = 0x40;
    public const byte GetPerf = 0x41;
    public const byte SetBootImage = 0x50;
    public const byte GetBootImage = 0x51;
    public const byte GetDisplay = 0x52;
    public const byte IoState = 0x53;
    public const byte OtaBegin = 0x60;
    public const byte OtaData = 0x61;
    public const byte OtaEnd = 0x62;
    public const byte OtaConfirm = 0x63;
    public const byte OtaRollback = 0x64;
    public const byte DebugSetBp = 0x70;
    public const byte DebugContinue = 0x71;
    public const byte DebugStep = 0x72;
    public const byte DebugStack = 0x73;
    public const byte DebugClearBp = 0x74;
    public const byte DebugState = 0x75;
    public const byte DebugLocals = 0x76;
    public const byte Reboot = 0x7F;
}

public sealed record RndpFrame(byte Code, byte[] Payload)
{
    public const byte StatusOk = 0x00;
    public const byte StatusErr = 0x01;

    /// <summary>
    /// Largest payload any command legitimately carries, and the sanity bound
    /// that lets a decoder tell a frame from a coincidence.
    /// </summary>
    public const uint MaxPayload = 8 * 1024 * 1024;

    public bool IsOk => Code == StatusOk;
    public string PayloadText => System.Text.Encoding.UTF8.GetString(Payload);

    /// <summary>CRC-16/CCITT-FALSE, identical to the firmware implementation.</summary>
    public static ushort Crc16(ReadOnlySpan<byte> data)
    {
        ushort crc = 0xFFFF;
        foreach (byte b in data)
        {
            crc ^= (ushort)(b << 8);
            for (int i = 0; i < 8; i++)
            {
                crc = (crc & 0x8000) != 0 ? (ushort)((crc << 1) ^ 0x1021) : (ushort)(crc << 1);
            }
        }
        return crc;
    }

    public byte[] Encode()
    {
        byte[] body = new byte[1 + 4 + Payload.Length];
        body[0] = Code;
        BitConverter.TryWriteBytes(body.AsSpan(1, 4), (uint)Payload.Length);
        Payload.CopyTo(body, 5);
        ushort crc = Crc16(body);
        byte[] frame = new byte[2 + body.Length + 2];
        frame[0] = 0x52;
        frame[1] = 0x4E;
        body.CopyTo(frame, 2);
        BitConverter.TryWriteBytes(frame.AsSpan(frame.Length - 2, 2), crc);
        return frame;
    }

    /// <summary>Decode one frame from the buffer; returns consumed byte count or 0 if incomplete.</summary>
    public static int TryDecode(ReadOnlySpan<byte> buffer, out RndpFrame? frame)
    {
        frame = null;
        if (buffer.Length < 9)
        {
            return 0;
        }
        if (buffer[0] != 0x52 || buffer[1] != 0x4E)
        {
            throw new InvalidDataException("bad RNDP magic");
        }
        byte code = buffer[2];
        uint len = BitConverter.ToUInt32(buffer.Slice(3, 4));
        if (len > MaxPayload)
        {
            // Not a frame: a stray "RN" in log text or line noise. Reported the
            // same way as a bad CRC so the caller can resync past it. Without
            // the bound, a length near uint.MaxValue casts to a negative int
            // and the payload slice throws instead.
            throw new InvalidDataException($"implausible RNDP payload length {len}");
        }
        int total = 2 + 1 + 4 + (int)len + 2;
        if (buffer.Length < total)
        {
            return 0;
        }
        byte[] payload = buffer.Slice(7, (int)len).ToArray();
        ushort got = BitConverter.ToUInt16(buffer.Slice(7 + (int)len, 2));
        ushort want = Crc16(buffer.Slice(2, 5 + (int)len));
        if (got != want)
        {
            throw new InvalidDataException($"RNDP crc mismatch: got {got:x4}, want {want:x4}");
        }
        frame = new RndpFrame(code, payload);
        return total;
    }
}
