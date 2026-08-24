using RustNet.Net;

namespace RustNet.Cloud;

/// <summary>
/// AWS IoT Core device client over MQTT. Production auth is mutual-TLS with
/// an X.509 device certificate (the TLS layer is the device integration
/// point); this client owns the topic conventions — telemetry pub/sub and
/// Device Shadow — and the connect/publish flow.
/// </summary>
public class AwsIotCore
{
    private readonly string _endpoint;
    private readonly string _thingName;

    public AwsIotCore(string endpoint, string thingName)
    {
        _endpoint = endpoint;
        _thingName = thingName;
    }

    /// <summary>Classic (unnamed) shadow update topic.</summary>
    public string ShadowUpdateTopic => $"$aws/things/{_thingName}/shadow/update";

    /// <summary>Shadow "delta" (desired != reported) subscribe topic.</summary>
    public string ShadowDeltaTopic => $"$aws/things/{_thingName}/shadow/update/delta";

    /// <summary>Shadow get/accepted topic.</summary>
    public string ShadowGetAcceptedTopic => $"$aws/things/{_thingName}/shadow/get/accepted";

    /// <summary>Wrap a reported-state object in the AWS shadow envelope.</summary>
    public static string ShadowReported(string reportedJson)
    {
        return string.Concat("{\"state\":{\"reported\":", reportedJson, "}}");
    }

    public bool Connect()
    {
        // TLS on 8883 with the device cert in a real deployment.
        return Mqtt.ConnectAuth($"{_endpoint}:8883", _thingName, "", "");
    }

    public void Publish(string topic, string json)
    {
        Mqtt.Publish(topic, json, 1);
    }

    /// <summary>Report device state to its shadow.</summary>
    public void UpdateShadow(string reportedJson)
    {
        Mqtt.Publish(ShadowUpdateTopic, ShadowReported(reportedJson), 1);
    }

    public void SubscribeShadowDelta()
    {
        Mqtt.Subscribe(ShadowDeltaTopic);
    }
}
