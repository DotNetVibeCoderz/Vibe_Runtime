using RustNet.Net;

namespace RustNet.Cloud;

/// <summary>
/// IFTTT Webhooks (Maker) trigger client. Fires an applet event with up to
/// three values. IFTTT's endpoint is HTTPS; this builds the correct
/// request (path + JSON body / query) and sends it via the HTTP client
/// (a TLS-capable build reaches maker.ifttt.com directly; otherwise point
/// it at a local relay).
/// </summary>
public class Ifttt
{
    private readonly string _key;
    private readonly string _host;

    public Ifttt(string webhookKey)
    {
        _key = webhookKey;
        _host = "maker.ifttt.com";
    }

    /// <summary>Override the host (e.g. a local HTTPS relay for testing).</summary>
    public Ifttt(string webhookKey, string host)
    {
        _key = webhookKey;
        _host = host;
    }

    public string Host => _host;

    /// <summary>Request path for an event: `/trigger/{event}/with/key/{key}`.</summary>
    public string Path(string eventName)
    {
        return $"/trigger/{eventName}/with/key/{_key}";
    }

    /// <summary>JSON body carrying the three IFTTT ingredient values.</summary>
    public static string Body(string value1, string value2, string value3)
    {
        string v1 = Escape(value1);
        string v2 = Escape(value2);
        string v3 = Escape(value3);
        return $"{{\"value1\":\"{v1}\",\"value2\":\"{v2}\",\"value3\":\"{v3}\"}}";
    }

    /// <summary>Fire the trigger with three values (GET query fallback that
    /// works without an HTTP POST intrinsic).</summary>
    public string Trigger(string eventName, string value1, string value2, string value3)
    {
        string v1 = RustNet.Security.Url.Encode(value1);
        string v2 = RustNet.Security.Url.Encode(value2);
        string v3 = RustNet.Security.Url.Encode(value3);
        string path = $"{Path(eventName)}?value1={v1}&value2={v2}&value3={v3}";
        return Http.Get(_host, path);
    }

    public string Trigger(string eventName)
    {
        return Http.Get(_host, Path(eventName));
    }

    private static string Escape(string s)
    {
        return s.Replace("\\", "\\\\").Replace("\"", "\\\"");
    }
}
