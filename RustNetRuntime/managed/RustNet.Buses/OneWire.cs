using System.Runtime.CompilerServices;

namespace RustNet.Buses;

/// <summary>
/// Dallas/Maxim 1-Wire bus master. Timing-critical bit banging happens in
/// the Rust HAL; managed code works at the byte/ROM level.
/// </summary>
public static class OneWire
{
    /// <summary>Bus reset; true when at least one slave answered presence.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool Reset(int bus);

    /// <summary>Raw ROM list, 8 bytes per device little-endian; prefer <see cref="Search"/>.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern byte[] SearchRaw(int bus);

    /// <summary>Address one slave (MATCH ROM).</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Select(int bus, long rom);

    /// <summary>Address all slaves (SKIP ROM).</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Skip(int bus);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Write(int bus, byte[] data);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern byte[] Read(int bus, int count);

    /// <summary>Enumerate slave ROM codes on the bus.</summary>
    public static long[] Search(int bus)
    {
        byte[] raw = SearchRaw(bus);
        long[] roms = new long[raw.Length / 8];
        for (int i = 0; i < roms.Length; i++)
        {
            long rom = 0;
            for (int b = 7; b >= 0; b--)
            {
                rom = (rom << 8) | raw[i * 8 + b];
            }
            roms[i] = rom;
        }
        return roms;
    }

    public static void WriteByte(int bus, int value)
    {
        byte[] one = new byte[1];
        one[0] = (byte)value;
        Write(bus, one);
    }
}
