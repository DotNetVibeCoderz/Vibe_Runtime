using System.Collections.Concurrent;

namespace RustNet.Deploy;

/// <summary>
/// A connected BLE GATT link: RNDP bytes are carried as writes to / notifications
/// from a characteristic, each bounded by the negotiated ATT MTU. A platform
/// provides an implementation (nRF, ESP32 GATT, WinRT/BlueZ host stack, …);
/// <see cref="BleTransport"/> layers the RNDP byte stream on top.
/// </summary>
public interface IBlePacketLink : IDisposable
{
    /// <summary>Maximum payload bytes per packet (ATT MTU minus headers).</summary>
    int Mtu { get; }
    /// <summary>Write one packet (already sized to at most <see cref="Mtu"/>).</summary>
    void Send(ReadOnlySpan<byte> packet);
    /// <summary>Receive one packet into <paramref name="buffer"/>; returns its
    /// length, or 0 on timeout.</summary>
    int Receive(byte[] buffer, int timeoutMs);
}

/// <summary>
/// Carries the RNDP byte stream over an MTU-limited BLE packet link: writes are
/// fragmented into ≤MTU packets and reads reassemble them back into a stream,
/// so <see cref="RndpClient"/> works over BLE unchanged. The radio itself is the
/// injected <see cref="IBlePacketLink"/>.
/// </summary>
public sealed class BleTransport(IBlePacketLink link) : IDeviceTransport
{
    private byte[] _rx = Array.Empty<byte>();
    private int _rxPos;
    private int _rxLen;

    public void Write(ReadOnlySpan<byte> data)
    {
        int mtu = Math.Max(1, link.Mtu);
        for (int offset = 0; offset < data.Length; offset += mtu)
        {
            link.Send(data.Slice(offset, Math.Min(mtu, data.Length - offset)));
        }
    }

    public int Read(byte[] buffer, int timeoutMs)
    {
        if (_rxPos >= _rxLen)
        {
            var packet = new byte[Math.Max(link.Mtu, 1)];
            int n = link.Receive(packet, timeoutMs);
            if (n <= 0)
            {
                return 0;
            }
            _rx = packet;
            _rxLen = n;
            _rxPos = 0;
        }
        int take = Math.Min(_rxLen - _rxPos, buffer.Length);
        Array.Copy(_rx, _rxPos, buffer, 0, take);
        _rxPos += take;
        return take;
    }

    public void Dispose() => link.Dispose();
}

/// <summary>
/// An in-memory <see cref="IBlePacketLink"/> pair for tests: two links wired to
/// each other's queues with a configurable MTU, exercising the fragmentation and
/// reassembly without a radio.
/// </summary>
public sealed class LoopbackBleLink : IBlePacketLink
{
    private readonly BlockingCollection<byte[]> _incoming;
    private readonly BlockingCollection<byte[]> _outgoing;
    public int Mtu { get; }

    private LoopbackBleLink(BlockingCollection<byte[]> incoming, BlockingCollection<byte[]> outgoing, int mtu)
    {
        _incoming = incoming;
        _outgoing = outgoing;
        Mtu = mtu;
    }

    public static (LoopbackBleLink A, LoopbackBleLink B) Pair(int mtu)
    {
        var ab = new BlockingCollection<byte[]>();
        var ba = new BlockingCollection<byte[]>();
        return (new LoopbackBleLink(ba, ab, mtu), new LoopbackBleLink(ab, ba, mtu));
    }

    public void Send(ReadOnlySpan<byte> packet)
    {
        if (packet.Length > Mtu)
        {
            throw new InvalidOperationException($"packet {packet.Length} exceeds MTU {Mtu}");
        }
        _outgoing.Add(packet.ToArray());
    }

    public int Receive(byte[] buffer, int timeoutMs)
    {
        if (_incoming.TryTake(out byte[]? packet, timeoutMs))
        {
            packet.CopyTo(buffer, 0);
            return packet.Length;
        }
        return 0;
    }

    public void Dispose()
    {
    }
}
