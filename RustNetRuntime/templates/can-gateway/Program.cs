using RustNet.Buses;

namespace __NAME__;

/// <summary>
/// Industrial gateway: bridges CAN frames to Modbus holding registers and
/// keeps them reachable over Ethernet. CAN runs in loopback here so the
/// demo is self-contained; drop loopback for a real bus.
/// </summary>
public static class Program
{
    public static void Main()
    {
        Console.WriteLine("can-gateway starting");

        Can.Init(0, 500000, true);
        Can.SetFilter(0, 0x100, 0x700); // accept 0x100..0x1FF
        bool eth = RustNet.Net.Ethernet.Up();
        Console.WriteLine(string.Concat("ethernet ip=", RustNet.Net.Ethernet.GetIp()));

        // Publish three sensor frames (loopback echoes them to our RX FIFO).
        for (int i = 0; i < 3; i++)
        {
            byte[] payload = new byte[2];
            payload[0] = (byte)(20 + i);
            payload[1] = (byte)i;
            Can.Write(0, 0x110 + i, payload);
        }

        // Bridge every pending frame into Modbus registers 200+.
        int slot = 200;
        while (Can.Available(0) > 0)
        {
            CanFrame frame = Can.Read(0);
            if (frame == null)
            {
                break;
            }
            int value = frame.Data.Length >= 2 ? (frame.Data[0] << 8) | frame.Data[1] : 0;
            Modbus.WriteRegister(1, slot, value);
            Console.WriteLine($"bridged can 0x{frame.Id:x} -> holding[{slot}] = {value}");
            slot = slot + 1;
        }

        int[] regs = Modbus.ReadHoldingRegisters(1, 200, 3);
        Console.WriteLine($"holding 200..202 = {regs[0]}, {regs[1]}, {regs[2]}");
        Console.WriteLine("can-gateway finished");
    }
}
