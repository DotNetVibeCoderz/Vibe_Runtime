using RustNet.Data;
using RustNet.Devices;
using RustNet.Serialization;

namespace __NAME__;

/// <summary>
/// Data logger: samples a DS18B20 on 1-Wire bus 0 every few seconds,
/// stores readings in an on-flash SQL database stamped by the RTC, and
/// prints a JSON summary. Watchdog guards the loop.
/// </summary>
public static class Program
{
    public static void Main()
    {
        Console.WriteLine("datalogger starting");

        Database db = Database.Open("/data/logger.db");
        db.Execute("CREATE TABLE IF NOT EXISTS samples (at INTEGER, centi_c INTEGER)");

        Ds18b20 sensor = Ds18b20.Find(0);
        if (sensor == null)
        {
            Console.WriteLine("no DS18B20 found on 1-wire bus 0");
            return;
        }

        RustNet.Sys.Watchdog.Start(10000);
        for (int i = 0; i < 5; i++)
        {
            int centi = sensor.ReadCentiCelsius();
            long at = RustNet.Sys.Rtc.Epoch();
            db.Execute($"INSERT INTO samples VALUES ({at}, {centi})");
            Console.WriteLine($"sample {i}: {centi} centi-C at {at}");
            RustNet.Sys.Watchdog.Feed();
            RustNet.Threading.Sleep.Ms(2000);
        }
        RustNet.Sys.Watchdog.Stop();

        // Summarize as JSON for upstream reporting.
        string count = db.Scalar("SELECT COUNT(*) FROM samples");
        string avg = db.Scalar("SELECT AVG(centi_c) FROM samples");
        JsonValue summary = JsonValue.NewObject();
        summary.Set("samples", count);
        summary.Set("avg_centi_c", avg);
        Console.WriteLine(summary.ToJson());
        db.Close();
    }
}
