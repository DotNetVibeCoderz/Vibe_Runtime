using RustNet.Hal;
using RustNet.Net;
using RustNet.Threading;

namespace __NAME__;

/// <summary>
/// WiFi + MQTT telemetry: connects to an access point, then publishes ADC
/// readings to an MQTT broker (e.g. mosquitto on localhost:1883).
/// </summary>
public static class Program
{
    public static void Main()
    {
        Console.WriteLine("__NAME__ wifi+mqtt telemetry");
        if (!Wifi.Connect("__NAME__-network", "change-me"))
        {
            Console.WriteLine("wifi connect failed");
            return;
        }
        Console.WriteLine("wifi connected");

        if (!Mqtt.Connect("127.0.0.1:1883", "__NAME__-device"))
        {
            Console.WriteLine("mqtt broker not reachable (start mosquitto and retry)");
            return;
        }
        Console.WriteLine("mqtt connected");
        Mqtt.Subscribe("__NAME__/cmd");

        for (int i = 1; i <= 10; i++)
        {
            int mv = Adc.ReadMillivolts(0);
            string payload = string.Concat("{\"sample\":", i.ToString(), ",\"millivolts\":", mv.ToString(), "}");
            Mqtt.Publish("__NAME__/telemetry", payload, 1);
            Console.WriteLine(string.Concat("published: ", payload));
            Sleep.Ms(2000);
        }
        Console.WriteLine("telemetry session complete");
    }
}
