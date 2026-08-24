using RustNet.Net;
using RustNet.Threading;

namespace __NAME__;

/// <summary>
/// Weather check over WiFi + HTTP. Point it at any endpoint that answers
/// "temp_c=NN;condition=Sunny" style lines (see README for a tiny local
/// server you can run for testing).
/// </summary>
public static class Program
{
    public static void Main()
    {
        Console.WriteLine("__NAME__ weather check");
        bool connected = Wifi.Connect("__NAME__-network", "change-me");
        if (!connected)
        {
            Console.WriteLine("wifi connect failed");
            return;
        }
        Console.WriteLine("wifi connected");

        for (int attempt = 1; attempt <= 3; attempt++)
        {
            string body = Http.Get("127.0.0.1:8085", "/weather");
            if (string.IsNullOrEmpty(body))
            {
                Console.WriteLine("empty response, retrying");
                Sleep.Ms(1000);
                continue;
            }
            Console.WriteLine(string.Concat("weather: ", body));
            string[] parts = body.Split(';');
            for (int i = 0; i < parts.Length; i++)
            {
                Console.WriteLine(string.Concat("  ", parts[i]));
            }
            return;
        }
        Console.WriteLine("no weather data after 3 attempts");
    }
}
