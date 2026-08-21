namespace CodeGen.Models;

/// <summary>Whether a board is reachable right now.</summary>
public enum DeviceState
{
    /// <summary>A port or probe matching this board is present.</summary>
    Connected,

    /// <summary>Nothing matching is attached.</summary>
    NotConnected,

    /// <summary>
    /// Something is attached but which board it is cannot be told apart.
    ///
    /// A CH340 bridge looks the same whether an ESP32 or an unrelated device is
    /// behind it, and a bare ST-Link says nothing about the target. Reporting
    /// this rather than guessing is the point — a wrong board identification
    /// leads to flashing the wrong image.
    /// </summary>
    Ambiguous,

    /// <summary>The tool needed to talk to this board is not installed.</summary>
    ToolMissing,
}

/// <summary>A serial port the host can see.</summary>
public sealed record SerialPortInfo(string PortName, string Description)
{
    public override string ToString() => $"{PortName} — {Description}";
}

/// <summary>What Device Status knows about one board at one moment.</summary>
public sealed record DeviceStatus
{
    public required Board Board { get; init; }
    public required DeviceState State { get; init; }

    /// <summary>The port or probe it was found on, when it was found.</summary>
    public string? Endpoint { get; init; }

    /// <summary>Why it is in this state, in one line.</summary>
    public required string Detail { get; init; }

    /// <summary>
    /// What the chip reported about itself, once something has actually talked
    /// to it. Null until a probe runs — an identity read costs a round trip and
    /// resets some boards, so it is never done as part of enumeration.
    /// </summary>
    public string? ChipReport { get; init; }

    /// <summary>The firmware image on disk for this board, if it has been built.</summary>
    public FirmwareInfo? Firmware { get; init; }

    public bool IsConnected => State == DeviceState.Connected;

    public string StateLabel => State switch
    {
        DeviceState.Connected => "connected",
        DeviceState.NotConnected => "not connected",
        DeviceState.Ambiguous => "possibly connected",
        DeviceState.ToolMissing => "tool missing",
        _ => State.ToString(),
    };
}

/// <summary>A built firmware image on disk.</summary>
public sealed record FirmwareInfo(string Path, long SizeBytes, DateTime BuiltUtc)
{
    public string SizeLabel => SizeBytes >= 1024 * 1024
        ? $"{SizeBytes / 1024.0 / 1024.0:0.0} MB"
        : $"{SizeBytes / 1024.0:0} KB";

    public string BuiltLabel => BuiltUtc.ToLocalTime().ToString("yyyy-MM-dd HH:mm");
}
