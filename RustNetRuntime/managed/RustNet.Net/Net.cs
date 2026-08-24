using System.Runtime.CompilerServices;

namespace RustNet.Net;

/// <summary>Wired Ethernet interface (DHCP by default).</summary>
public static class Ethernet
{
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool Up();

    /// <summary>Bring up with a static address instead of DHCP.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool Up(string staticIp, string gateway);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Down();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string GetIp();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool IsUp();
}

/// <summary>Point-to-point link over a serial modem (PPP).</summary>
public static class Ppp
{
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool Up(int uartPort, string username, string password);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Down();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string GetIp();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool IsUp();
}

/// <summary>LTE/NB-IoT cellular modem.</summary>
public static class Cellular
{
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool Up(string apn, string username, string password);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Down();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string GetIp();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool IsUp();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string GetOperator();

    /// <summary>Signal strength in dBm (negative; closer to 0 is better).</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern int GetRssi();
}

public static class Wifi
{
    /// <summary>Connect to an access point. Returns true when associated.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool Connect(string ssid, string psk);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool IsConnected();

    /// <summary>
    /// The network actually associated, as reported by the interface — not an
    /// echo of what <see cref="Connect"/> was given. On targets where the
    /// firmware joins the radio at boot (ESP32), this is the only way for an
    /// app to learn the real SSID. Empty when not associated.
    /// </summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string GetSsid();

    /// <summary>
    /// Current IPv4 address as "a.b.c.d", empty while unassigned. Throws if
    /// the board exposes no WiFi interface.
    /// </summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string GetIp();

    /// <summary>
    /// Leave the current network. Harmless when not associated.
    /// </summary>
    /// <remarks>
    /// Not just the inverse of <see cref="Connect"/>: on battery-powered
    /// boards an associated radio is the largest continuous draw there is, so
    /// an app that wakes, reports and sleeps wants to put it down explicitly
    /// rather than wait for the association to lapse.
    /// </remarks>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Disconnect();
}

/// <summary>MQTT 3.1.1 client (single connection per app).</summary>
public static class Mqtt
{
    /// <summary>Connect to host:port with a client id.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool Connect(string address, string clientId);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Publish(string topic, string payload, int qos);

    /// <summary>Connect with MQTT username/password (cloud IoT auth).
    /// Empty strings mean "omit that field".</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool ConnectAuth(string address, string clientId, string username, string password);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Subscribe(string topic);

    /// <summary>Block until a message arrives; returns "topic\npayload".</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string Poll();
}

public static class Http
{
    /// <summary>GET http://address/path and return the body as text.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string Get(string address, string path);
}
