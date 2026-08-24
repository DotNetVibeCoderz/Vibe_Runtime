using RustNet.Buses;
using RustNet.Hal;

namespace RustNet.Devices;

/// <summary>DS18B20 1-Wire temperature sensor.</summary>
public class Ds18b20
{
    private readonly int _bus;
    private readonly long _rom;

    public Ds18b20(int bus, long rom)
    {
        _bus = bus;
        _rom = rom;
    }

    /// <summary>First DS18B20 on the bus (family code 0x28), or null.</summary>
    public static Ds18b20 Find(int bus)
    {
        if (!OneWire.Reset(bus))
        {
            return null;
        }
        long[] roms = OneWire.Search(bus);
        for (int i = 0; i < roms.Length; i++)
        {
            if ((roms[i] & 0xFF) == 0x28)
            {
                return new Ds18b20(bus, roms[i]);
            }
        }
        return null;
    }

    /// <summary>Temperature in hundredths of a degree C (2550 = 25.50 C).</summary>
    public int ReadCentiCelsius()
    {
        OneWire.Reset(_bus);
        OneWire.Select(_bus, _rom);
        OneWire.WriteByte(_bus, 0x44); // CONVERT T
        OneWire.Reset(_bus);
        OneWire.Select(_bus, _rom);
        OneWire.WriteByte(_bus, 0xBE); // READ SCRATCHPAD
        byte[] sp = OneWire.Read(_bus, 9);
        int raw = sp[0] | (sp[1] << 8);
        if (raw > 32767)
        {
            raw = raw - 65536;
        }
        return raw * 100 / 16;
    }
}

/// <summary>HC-SR04 ultrasonic distance sensor via PulseFeedback.</summary>
public class HcSr04
{
    private readonly int _pin;

    public HcSr04(int pin)
    {
        _pin = pin;
    }

    /// <summary>Distance in millimeters (0 = no echo). Speed of sound: 343 m/s.</summary>
    public int ReadMillimeters()
    {
        int echoUs = Signal.PulseFeedback(_pin, true, 10, 30000);
        return echoUs * 343 / 2000;
    }
}
