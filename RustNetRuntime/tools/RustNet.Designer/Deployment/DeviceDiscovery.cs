using System;
using System.Collections.Generic;
using System.IO.Ports;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using RustNet.Deploy;

namespace RustNet.Designer.Deployment;

/// <summary>One place the Designer can deploy to.</summary>
/// <param name="Spec">RNDP device spec, e.g. <c>serial:COM5</c> or <c>tcp:127.0.0.1:7878</c>.</param>
/// <param name="Label">What the picker shows.</param>
/// <param name="Chip">Chip family reported by the device, or empty if unprobed.</param>
/// <param name="Board">Board name reported by the device, or empty.</param>
public sealed record DeviceTarget(string Spec, string Label, string Chip = "", string Board = "")
{
    public bool Answered => Chip.Length > 0;

    public override string ToString() => Label;
}

/// <summary>
/// Finds devices to deploy to. Candidates are the local virtual device and every
/// serial port; a candidate becomes a real target once it answers an RNDP
/// <c>info</c>. The chip family comes from that answer, which matters because
/// signing has to name the chip the device will verify against.
///
/// Probing is sequential with a short timeout: opening a serial port that
/// something else already holds throws, and a port with no RustNet on it simply
/// never answers.
/// </summary>
public static class DeviceDiscovery
{
    /// <summary>The local virtual device (<c>rustnet-firmware</c>), always offered.</summary>
    public const string VirtualDeviceSpec = "tcp:127.0.0.1:7878";

    private const int ProbeTimeoutMs = 1200;

    /// <summary>Everything worth trying, before any of it has answered.</summary>
    public static List<string> Candidates()
    {
        var specs = new List<string> { VirtualDeviceSpec };
        string[] ports;
        try
        {
            ports = SerialPort.GetPortNames();
        }
        catch (Exception)
        {
            ports = Array.Empty<string>();
        }
        Array.Sort(ports, StringComparer.OrdinalIgnoreCase);
        foreach (string port in ports)
        {
            specs.Add("serial:" + port);
        }
        return specs;
    }

    /// <summary>
    /// Probe every candidate and return the ones that answered, most useful
    /// first. <paramref name="log"/> gets a line per candidate so the output pane
    /// shows what was tried.
    /// </summary>
    public static Task<List<DeviceTarget>> ScanAsync(Action<string> log, CancellationToken cancellationToken)
        => Task.Run(() =>
        {
            var found = new List<DeviceTarget>();
            foreach (string spec in Candidates())
            {
                cancellationToken.ThrowIfCancellationRequested();
                DeviceTarget? target = Probe(spec, log);
                if (target != null)
                {
                    found.Add(target);
                }
            }
            if (found.Count == 0)
            {
                log("No device answered. Start the virtual device with "
                    + "`cargo run -p rustnet-firmware -- --ephemeral`, or plug a board in.");
            }
            return found;
        }, cancellationToken);

    /// <summary>Ask one spec who it is. Returns null when nothing answered.</summary>
    public static DeviceTarget? Probe(string spec, Action<string> log)
    {
        try
        {
            // Probing walks every candidate port, so the readiness wait is
            // capped at the probe budget rather than the default boot window.
            using RndpClient client = RndpClient.Connect(spec, ProbeTimeoutMs);
            RndpFrame frame = client.Call(Cmd.Info, Array.Empty<byte>(), ProbeTimeoutMs);
            if (!frame.IsOk)
            {
                log($"{spec}: refused ({frame.PayloadText})");
                return null;
            }
            (string chip, string board) = ParseInfo(frame.PayloadText);
            log($"{spec}: {board} (chip {chip})");
            return new DeviceTarget(spec, $"{Short(spec)} — {board}", chip, board);
        }
        catch (Exception ex)
        {
            log($"{spec}: {ex.Message}");
            return null;
        }
    }

    private static (string Chip, string Board) ParseInfo(string json)
    {
        try
        {
            using JsonDocument doc = JsonDocument.Parse(json);
            string chip = Text(doc, "chip");
            string board = Text(doc, "board");
            return (chip.Length > 0 ? chip : "any", board.Length > 0 ? board : "unknown board");
        }
        catch (JsonException)
        {
            return ("any", "unknown board");
        }

        static string Text(JsonDocument d, string name)
            => d.RootElement.TryGetProperty(name, out JsonElement v) && v.ValueKind == JsonValueKind.String
                ? v.GetString() ?? "" : "";
    }

    private static string Short(string spec)
        => spec == VirtualDeviceSpec ? "virtual device" : spec.Replace("serial:", "").Replace("tcp:", "");
}
