using RustNet.Hal;

namespace RustNet.Devices;

/// <summary>TMP36-style analog temperature sensor on an ADC channel.</summary>
public class AnalogTemperatureSensor
{
    private readonly int _channel;

    public AnalogTemperatureSensor(int adcChannel)
    {
        _channel = adcChannel;
    }

    /// <summary>Temperature in tenths of a degree C (avoids float where unneeded).</summary>
    public int ReadDeciCelsius()
    {
        int mv = Adc.ReadMillivolts(_channel);
        // TMP36: 750 mV at 25.0 C, 10 mV per degree.
        return 250 + (mv - 750);
    }

    public double ReadCelsius() => ReadDeciCelsius() / 10.0;
}

/// <summary>Simple photoresistor / moisture-style analog sensor.</summary>
public class AnalogSensor
{
    private readonly int _channel;

    public AnalogSensor(int adcChannel)
    {
        _channel = adcChannel;
    }

    public int ReadRaw() => Adc.ReadRaw(_channel);

    /// <summary>Reading as 0..100 percent of full scale (12-bit).</summary>
    public int ReadPercent() => Adc.ReadRaw(_channel) * 100 / 4095;
}

/// <summary>
/// BMP280 pressure/temperature sensor over I2C (forced-mode, simplified
/// integer compensation).
/// </summary>
public class Bmp280
{
    private readonly int _bus;
    private readonly int _address;

    public Bmp280(int bus) : this(bus, 0x76)
    {
    }

    public Bmp280(int bus, int address)
    {
        _bus = bus;
        _address = address;
        // ctrl_meas: temp oversampling x1, pressure x1, forced mode.
        byte[] cmd = new byte[2];
        cmd[0] = 0xF4;
        cmd[1] = 0x25;
        I2c.Write(_bus, _address, cmd);
    }

    public bool IsPresent()
    {
        byte[] reg = new byte[1];
        reg[0] = 0xD0; // chip id register
        I2c.Write(_bus, _address, reg);
        byte[] id = I2c.Read(_bus, _address, 1);
        return id.Length == 1 && (id[0] == 0x58 || id[0] == 0x60);
    }

    /// <summary>Raw 20-bit pressure reading (needs calibration applied by caller).</summary>
    public int ReadRawPressure()
    {
        byte[] reg = new byte[1];
        reg[0] = 0xF7;
        I2c.Write(_bus, _address, reg);
        byte[] d = I2c.Read(_bus, _address, 3);
        return (d[0] << 12) | (d[1] << 4) | (d[2] >> 4);
    }
}

/// <summary>MPU6050 accelerometer/gyro over I2C (raw axis reads).</summary>
public class Mpu6050
{
    private readonly int _bus;
    private readonly int _address;

    public Mpu6050(int bus) : this(bus, 0x68)
    {
    }

    public Mpu6050(int bus, int address)
    {
        _bus = bus;
        _address = address;
        // Wake up (PWR_MGMT_1 = 0).
        byte[] cmd = new byte[2];
        cmd[0] = 0x6B;
        cmd[1] = 0x00;
        I2c.Write(_bus, _address, cmd);
    }

    /// <summary>Raw 16-bit acceleration for axis 0=X 1=Y 2=Z.</summary>
    public int ReadAccelRaw(int axis)
    {
        byte[] reg = new byte[1];
        reg[0] = (byte)(0x3B + axis * 2);
        I2c.Write(_bus, _address, reg);
        byte[] d = I2c.Read(_bus, _address, 2);
        int value = (d[0] << 8) | d[1];
        if (value > 32767)
        {
            value = value - 65536;
        }
        return value;
    }
}

/// <summary>
/// NMEA 0183 sentence parser for GPS modules (pure managed code —
/// works with any UART/host transport that hands it lines).
/// </summary>
public class GpsNmeaParser
{
    public bool HasFix;
    public double Latitude;
    public double Longitude;
    public int Satellites;

    /// <summary>Feed one "$GPGGA,..." sentence. Returns true if position updated.</summary>
    public bool Parse(string sentence)
    {
        if (!sentence.StartsWith("$GPGGA") && !sentence.StartsWith("$GNGGA"))
        {
            return false;
        }
        string[] parts = sentence.Split(',');
        if (parts.Length < 8)
        {
            return false;
        }
        string quality = parts[6];
        if (quality == "0" || quality.Length == 0)
        {
            HasFix = false;
            return false;
        }
        double rawLat = ParseDouble(parts[2]);
        double rawLon = ParseDouble(parts[4]);
        if (rawLat == 0.0 && rawLon == 0.0)
        {
            return false;
        }
        Latitude = NmeaToDegrees(rawLat);
        if (parts[3] == "S")
        {
            Latitude = -Latitude;
        }
        Longitude = NmeaToDegrees(rawLon);
        if (parts[5] == "W")
        {
            Longitude = -Longitude;
        }
        Satellites = ParseInt(parts[7]);
        HasFix = true;
        return true;
    }

    private static double NmeaToDegrees(double raw)
    {
        // ddmm.mmmm -> decimal degrees
        int degrees = (int)(raw / 100.0);
        double minutes = raw - degrees * 100.0;
        return degrees + minutes / 60.0;
    }

    private static double ParseDouble(string s)
    {
        // Minimal parser (no culture, no exponents) for runtime portability.
        double result = 0.0;
        double frac = 0.0;
        double scale = 0.1;
        bool inFrac = false;
        int i = 0;
        while (i < s.Length)
        {
            char c = s[i];
            if (c == '.')
            {
                inFrac = true;
            }
            else if (c >= '0' && c <= '9')
            {
                if (inFrac)
                {
                    frac = frac + (c - '0') * scale;
                    scale = scale * 0.1;
                }
                else
                {
                    result = result * 10.0 + (c - '0');
                }
            }
            else
            {
                return 0.0;
            }
            i = i + 1;
        }
        return result + frac;
    }

    private static int ParseInt(string s)
    {
        int result = 0;
        int i = 0;
        while (i < s.Length)
        {
            char c = s[i];
            if (c < '0' || c > '9')
            {
                return result;
            }
            result = result * 10 + (c - '0');
            i = i + 1;
        }
        return result;
    }
}

/// <summary>8x8 LED matrix over I2C (HT16K33-style controller).</summary>
public class LedMatrix
{
    private readonly int _bus;
    private readonly int _address;
    private readonly byte[] _rows;

    public LedMatrix(int bus) : this(bus, 0x70)
    {
    }

    public LedMatrix(int bus, int address)
    {
        _bus = bus;
        _address = address;
        _rows = new byte[8];
        byte[] on = new byte[1];
        on[0] = 0x21; // oscillator on
        I2c.Write(_bus, _address, on);
        on[0] = 0x81; // display on, no blink
        I2c.Write(_bus, _address, on);
    }

    public void SetPixel(int x, int y, bool on)
    {
        if (x < 0 || x > 7 || y < 0 || y > 7)
        {
            return;
        }
        if (on)
        {
            _rows[y] = (byte)(_rows[y] | (1 << x));
        }
        else
        {
            _rows[y] = (byte)(_rows[y] & ~(1 << x));
        }
    }

    public void Clear()
    {
        int i = 0;
        while (i < 8)
        {
            _rows[i] = 0;
            i = i + 1;
        }
    }

    public void Flush()
    {
        byte[] frame = new byte[17];
        frame[0] = 0x00;
        int i = 0;
        while (i < 8)
        {
            frame[1 + i * 2] = _rows[i];
            frame[2 + i * 2] = 0;
            i = i + 1;
        }
        I2c.Write(_bus, _address, frame);
    }
}
