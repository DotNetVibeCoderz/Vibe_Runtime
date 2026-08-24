using RustNet.Net;
using RustNet.Security;

namespace RustNet.Cloud;

/// <summary>
/// Google Cloud IoT Core device client over MQTT. Auth is a JWT in the MQTT
/// password field. This client owns the topic conventions and the JWT
/// header/claims assembly (base64url). Production Google projects require an
/// RS256/ES256 signature over the device private key (that signing step is
/// the device integration point); an HS256 JWT is provided for brokers/
/// gateways that accept a shared secret and for full on-device testing.
/// </summary>
public class GoogleIotCore
{
    private readonly string _projectId;
    private readonly string _region;
    private readonly string _registryId;
    private readonly string _deviceId;

    public GoogleIotCore(string projectId, string region, string registryId, string deviceId)
    {
        _projectId = projectId;
        _region = region;
        _registryId = registryId;
        _deviceId = deviceId;
    }

    /// <summary>The MQTT client id Google derives from the device path.</summary>
    public string ClientId =>
        $"projects/{_projectId}/locations/{_region}/registries/{_registryId}/devices/{_deviceId}";

    public string EventsTopic => $"/devices/{_deviceId}/events";
    public string StateTopic => $"/devices/{_deviceId}/state";
    public string ConfigTopic => $"/devices/{_deviceId}/config";

    /// <summary>base64url without padding (JWT segment encoding).</summary>
    public static string Base64Url(byte[] data)
    {
        string b64 = System.Convert.ToBase64String(data);
        b64 = b64.Replace('+', '-').Replace('/', '_');
        return b64.TrimEnd('=');
    }

    public static string Base64Url(string s)
    {
        return Base64Url(System.Text.Encoding.UTF8.GetBytes(s));
    }

    /// <summary>The signing input `base64url(header).base64url(claims)`.</summary>
    public string JwtSigningInput(long iatEpoch, long expEpoch)
    {
        string header = "{\"alg\":\"HS256\",\"typ\":\"JWT\"}";
        string claims = $"{{\"iat\":{iatEpoch},\"exp\":{expEpoch},\"aud\":\"{_projectId}\"}}";
        return string.Concat(Base64Url(header), ".", Base64Url(claims));
    }

    /// <summary>Build an HS256 JWT (shared-secret variant) for the password.</summary>
    public string CreateJwtHs256(byte[] secret, long iatEpoch, long expEpoch)
    {
        string signingInput = JwtSigningInput(iatEpoch, expEpoch);
        // Hmac.Sha256Base64 returns standard base64; convert to base64url.
        string sigStd = Hmac.Sha256Base64(secret, signingInput);
        string sigUrl = sigStd.Replace('+', '-').Replace('/', '_').TrimEnd('=');
        return string.Concat(signingInput, ".", sigUrl);
    }

    public bool Connect(byte[] jwtSecret, long iatEpoch, long expEpoch)
    {
        string jwt = CreateJwtHs256(jwtSecret, iatEpoch, expEpoch);
        // Username is ignored by Google; the JWT goes in the password.
        return Mqtt.ConnectAuth("mqtt.googleapis.com:8883", ClientId, "unused", jwt);
    }

    public void SendTelemetry(string json)
    {
        Mqtt.Publish(EventsTopic, json, 1);
    }

    public void SubscribeConfig()
    {
        Mqtt.Subscribe(ConfigTopic);
    }
}
