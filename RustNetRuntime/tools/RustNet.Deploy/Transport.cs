using System.IO.Ports;
using System.Net.Sockets;

namespace RustNet.Deploy;

/// <summary>Byte pipe to a device: TCP for the virtual device, serial for hardware.</summary>
public interface IDeviceTransport : IDisposable
{
    void Write(ReadOnlySpan<byte> data);
    /// <summary>Read available bytes (blocks until at least one or timeout). Returns 0 on timeout.</summary>
    int Read(byte[] buffer, int timeoutMs);

    /// <summary>
    /// True when opening the link may reboot the device, so the first request
    /// can be lost and has to be retried. Most USB-serial bridges wire DTR/RTS
    /// to reset/boot pins (that is how the ESP32 and K210 ROM loaders are
    /// entered), which means merely opening the port restarts the firmware.
    /// </summary>
    bool ResetsDeviceOnOpen => false;

    /// <summary>
    /// Restart the device into its application, for links that can. Used only
    /// as a recovery step when a device does not answer at all — a board can be
    /// left sitting in its ROM loader, or held in reset by a tool that exited
    /// without idling the control lines.
    /// </summary>
    void ResetIntoApplication() { }
}

public sealed class TcpTransport : IDeviceTransport
{
    private readonly TcpClient _client;
    private readonly NetworkStream _stream;

    public TcpTransport(string host, int port)
    {
        _client = new TcpClient();
        _client.Connect(host, port);
        _stream = _client.GetStream();
    }

    public void Write(ReadOnlySpan<byte> data) => _stream.Write(data);

    public int Read(byte[] buffer, int timeoutMs)
    {
        _stream.ReadTimeout = timeoutMs;
        try
        {
            return _stream.Read(buffer, 0, buffer.Length);
        }
        catch (IOException)
        {
            return 0;
        }
    }

    public void Dispose()
    {
        _stream.Dispose();
        _client.Dispose();
    }
}

public sealed class SerialTransport : IDeviceTransport
{
    private readonly SerialPort _port;

    public SerialTransport(string portName, int baud = 115200)
    {
        _port = new SerialPort(portName, baud, Parity.None, 8, StopBits.One);
        _port.Open();
        // Both lines idle: on the boards that wire them to reset/boot pins,
        // asserted DTR is what *holds* the device down. A tool that exited
        // without clearing them would otherwise leave the board unreachable.
        _port.DtrEnable = false;
        _port.RtsEnable = false;
    }

    public bool ResetsDeviceOnOpen => true;

    /// <summary>
    /// Pulse the device's reset and let it come up running its application.
    /// <para>
    /// On the K210's openec bridge reset is asserted whenever DTR and RTS
    /// <em>differ</em>, and <em>which</em> line was raised selects the boot
    /// mode: pulsing DTR with RTS idle boots the application, while pulsing RTS
    /// is precisely how kflash enters the ROM loader. Getting that backwards
    /// leaves the board in an ISP that answers nothing, which looks exactly
    /// like the dead device this method exists to rescue.
    /// </para>
    /// <para>
    /// On an ESP32 the same pulse is a no-op — there DTR drives IO0 and EN is
    /// left alone — so a working board is never disturbed by the attempt.
    /// </para>
    /// </summary>
    public void ResetIntoApplication()
    {
        _port.DtrEnable = false;
        _port.RtsEnable = false;
        Thread.Sleep(50);
        _port.DtrEnable = true;    // reset asserted, boot pin idle
        Thread.Sleep(100);
        _port.DtrEnable = false;   // released: boots into the application
        Thread.Sleep(50);
        _port.DiscardInBuffer();
    }

    public void Write(ReadOnlySpan<byte> data) => _port.BaseStream.Write(data);

    public int Read(byte[] buffer, int timeoutMs)
    {
        _port.ReadTimeout = timeoutMs;
        try
        {
            return _port.BaseStream.Read(buffer, 0, buffer.Length);
        }
        catch (TimeoutException)
        {
            return 0;
        }
    }

    public void Dispose() => _port.Dispose();
}

/// <summary>
/// Parses "tcp:host:port", "tcp:port", or "serial:COM3[:baud]" device
/// specifiers. Default is the local virtual device.
/// </summary>
public static class TransportFactory
{
    public const string DefaultSpec = "tcp:127.0.0.1:7878";

    /// <summary>
    /// Opens a BLE GATT link to the device at the address in a `ble:&lt;addr&gt;`
    /// spec. A platform with a BLE host stack registers this; without it, BLE
    /// specs are rejected with a clear message (the wire framing works — only the
    /// radio binding is platform-specific).
    /// </summary>
    public static Func<string, IBlePacketLink>? BleLinkProvider { get; set; }

    public static IDeviceTransport Open(string? spec)
    {
        spec = string.IsNullOrWhiteSpace(spec) ? DefaultSpec : spec;
        string[] parts = spec.Split(':');
        return parts[0].ToLowerInvariant() switch
        {
            "tcp" when parts.Length == 3 => new TcpTransport(parts[1], int.Parse(parts[2])),
            "tcp" when parts.Length == 2 => new TcpTransport("127.0.0.1", int.Parse(parts[1])),
            "serial" when parts.Length >= 2 => new SerialTransport(
                parts[1], parts.Length > 2 ? int.Parse(parts[2]) : 115200),
            "ble" when parts.Length >= 2 => OpenBle(spec[(spec.IndexOf(':') + 1)..]),
            _ => throw new ArgumentException(
                $"bad device spec '{spec}' (use tcp:host:port, serial:COM3[:baud] or ble:<address>)"),
        };
    }

    private static IDeviceTransport OpenBle(string address)
    {
        if (BleLinkProvider is null)
        {
            throw new NotSupportedException(
                "BLE transport needs a platform BLE backend — set TransportFactory.BleLinkProvider");
        }
        return new BleTransport(BleLinkProvider(address));
    }
}
