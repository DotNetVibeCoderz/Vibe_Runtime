using RustNet.Net;
using RustNet.Security;

namespace RustNet.Cloud;

/// <summary>
/// Azure IoT Hub device client over MQTT with SAS-token authentication.
///
/// The token/topic assembly is separated from the network calls so it can
/// be unit-tested against Azure's documented format. Live connection needs
/// a TLS-capable build (port 8883); the plaintext virtual device exercises
/// the token + topic construction and the connect attempt.
/// </summary>
public class AzureIotHub
{
    private const string ApiVersion = "2021-04-12";

    private readonly string _host;
    private readonly string _deviceId;

    public AzureIotHub(string hostName, string deviceId)
    {
        _host = hostName;
        _deviceId = deviceId;
    }

    /// <summary>`{host}/devices/{deviceId}` — the SAS resource URI.</summary>
    public string ResourceUri => $"{_host}/devices/{_deviceId}";

    /// <summary>MQTT username Azure expects.</summary>
    public string Username => $"{_host}/{_deviceId}/?api-version={ApiVersion}";

    /// <summary>Telemetry (device-to-cloud) publish topic.</summary>
    public string TelemetryTopic => $"devices/{_deviceId}/messages/events/";

    /// <summary>Cloud-to-device subscribe topic filter.</summary>
    public string CommandTopic => $"devices/{_deviceId}/messages/devicebound/#";

    /// <summary>The string signed for a SAS token: `{urlenc(uri)}\n{expiry}`.</summary>
    public static string StringToSign(string resourceUri, long expiryEpoch)
    {
        return string.Concat(Url.Encode(resourceUri), "\n", expiryEpoch.ToString());
    }

    /// <summary>Assemble a SAS token from an already-computed signature.</summary>
    public static string BuildSasToken(string resourceUri, string signatureBase64, long expiryEpoch)
    {
        string sr = Url.Encode(resourceUri);
        string sig = Url.Encode(signatureBase64);
        return $"SharedAccessSignature sr={sr}&sig={sig}&se={expiryEpoch}";
    }

    /// <summary>Full SAS token, signing the resource URI with the device key.</summary>
    public string CreateSasToken(byte[] deviceKey, long expiryEpoch)
    {
        string sig = Hmac.Sha256Base64(deviceKey, StringToSign(ResourceUri, expiryEpoch));
        return BuildSasToken(ResourceUri, sig, expiryEpoch);
    }

    /// <summary>Connect over MQTT. deviceKey is the base64 primary key bytes.</summary>
    public bool Connect(byte[] deviceKey, long expiryEpoch)
    {
        string token = CreateSasToken(deviceKey, expiryEpoch);
        // Azure requires TLS on 8883; the address is chosen by the caller's
        // build (plaintext virtual device uses :1883 and will be refused).
        return Mqtt.ConnectAuth($"{_host}:8883", _deviceId, Username, token);
    }

    public void SendTelemetry(string json)
    {
        Mqtt.Publish(TelemetryTopic, json, 1);
    }

    public void SubscribeCommands()
    {
        Mqtt.Subscribe(CommandTopic);
    }
}
