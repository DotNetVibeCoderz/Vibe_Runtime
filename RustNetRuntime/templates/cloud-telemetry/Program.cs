using RustNet.Cloud;
using RustNet.Serialization;

namespace __NAME__;

/// <summary>
/// Cloud telemetry: reads an ADC sensor and publishes JSON to Azure IoT
/// Hub over MQTT (swap in AwsIotCore / GoogleIotCore / Ifttt as needed).
/// A TLS-capable build reaches the real hub; the virtual device exercises
/// the SAS-token signing and payload path.
/// </summary>
public static class Program
{
    // Fill these in for your IoT Hub (device primary key is base64).
    private const string HubHost = "my-hub.azure-devices.net";
    private const string DeviceId = "device-01";
    private const string DeviceKeyBase64 = "cHV0LXlvdXItYmFzZTY0LWtleQ==";

    public static void Main()
    {
        Console.WriteLine("cloud-telemetry starting");

        AzureIotHub hub = new AzureIotHub(HubHost, DeviceId);
        byte[] key = System.Convert.FromBase64String(DeviceKeyBase64);
        long expiry = RustNet.Sys.Rtc.Epoch() + 3600;

        // Build the SAS token (HMAC-SHA256 on the device) and connect.
        bool connected = hub.Connect(key, expiry);
        Console.WriteLine(string.Concat("connected=", connected.ToString()));

        for (int i = 0; i < 5; i++)
        {
            int mv = RustNet.Hal.Adc.ReadMillivolts(0);
            JsonValue msg = JsonValue.NewObject();
            msg.Set("device", DeviceId);
            msg.Set("seq", i);
            msg.Set("adc_mv", mv);
            string payload = msg.ToJson();

            if (connected)
            {
                hub.SendTelemetry(payload);
            }
            Console.WriteLine(string.Concat("telemetry ", i.ToString(), ": ", payload));
            RustNet.Threading.Sleep.Ms(1000);
        }

        Console.WriteLine("cloud-telemetry done");
    }
}
