using RustNet.Devices;
using RustNet.IO;
using RustNet.Threading;

namespace __NAME__;

/// <summary>
/// IoT sensor logger: samples a TMP36 temperature sensor on ADC channel 0
/// every 2 seconds and appends CSV rows to /data/temperature.csv.
/// </summary>
public static class Program
{
    public static void Main()
    {
        Console.WriteLine("__NAME__ sensor logger starting");
        AnalogTemperatureSensor sensor = new AnalogTemperatureSensor(0);
        FileSystem.WriteAllText("/data/temperature.csv", "sample,deci_celsius\n");

        for (int sample = 1; sample <= 10; sample++)
        {
            int deci = sensor.ReadDeciCelsius();
            string row = string.Concat(sample.ToString(), ",", deci.ToString(), "\n");
            FileSystem.AppendText("/data/temperature.csv", row);
            Console.WriteLine(string.Concat("sample ", sample.ToString(), ": ", deci.ToString(), " dC"));
            Sleep.Ms(2000);
        }

        RustNet.Diagnostics.Log.Info("sensor logging finished");
        Console.WriteLine(FileSystem.ReadAllText("/data/temperature.csv"));
    }
}
