using System.Text.RegularExpressions;
using CodeGen.Models;

namespace CodeGen.Services;

/// <summary>
/// Talking to development boards: finding them, reporting what they are, and
/// putting firmware on them.
///
/// Everything here shells out to the tool that owns the transport —
/// <c>espflash</c>, <c>probe-rs</c>, <c>dfu-util</c> — rather than
/// reimplementing a bootloader protocol. That is not laziness: those tools know
/// the chip-specific reset dances, and a wrong guess about which one a board
/// needs writes an image to the wrong place.
///
/// Two rules run through this file:
///
/// <list type="bullet">
/// <item>**Enumeration never touches the board.** Listing ports is passive.
/// Reading a chip's identity costs a round trip and resets some parts, so it
/// only happens when the user asks for it.</item>
/// <item>**Ambiguity is reported, not resolved.** A CH340 bridge looks
/// identical whether an ESP32 or a sewing machine is behind it. Saying
/// "possibly connected" is more useful than a confident wrong answer, because
/// the next step writes to flash.</item>
/// </list>
/// </summary>
public sealed partial class DeviceService(ProcessRunner runner)
{
    /// <summary>Where the repository root is, for locating firmware crates.</summary>
    public string? RepositoryRoot { get; set; }

    // ── tools ───────────────────────────────────────────────────────────

    private readonly Dictionary<string, bool> _toolCache = [];

    /// <summary>Whether an external tool is on PATH.</summary>
    ///
    /// <remarks>
    /// Cached: this runs on every status refresh, and a process launch per
    /// board per refresh is enough to make the panel feel slow.
    /// </remarks>
    public async Task<bool> HasToolAsync(string tool, CancellationToken cancellationToken = default)
    {
        if (_toolCache.TryGetValue(tool, out var known)) return known;

        bool present;
        try
        {
            var result = await runner
                .RunAsync(tool, ["--version"], null, null, null, cancellationToken)
                .ConfigureAwait(false);
            present = result.ExitCode == 0;
        }
        catch
        {
            present = false;
        }
        _toolCache[tool] = present;
        return present;
    }

    /// <summary>Forgets cached tool lookups, so installing one takes effect.</summary>
    public void ForgetTools() => _toolCache.Clear();

    public static string ToolFor(FlashTool flash) => flash switch
    {
        FlashTool.EspFlash => "espflash",
        FlashTool.ProbeRs => "probe-rs",
        FlashTool.DfuUtil => "dfu-util",
        FlashTool.Uf2 => "cargo",
        _ => "cargo",
    };

    // ── enumeration ─────────────────────────────────────────────────────

    /// <summary>
    /// Serial ports the host can see, with a description where the platform
    /// offers one.
    ///
    /// Windows goes through PowerShell's PnP list because it carries the
    /// friendly name — "USB-SERIAL CH340 (COM18)" — which is what lets a user
    /// tell two boards apart. Elsewhere the device node is the whole answer.
    /// </summary>
    public async Task<IReadOnlyList<SerialPortInfo>> ListSerialPortsAsync(
        CancellationToken cancellationToken = default)
    {
        if (OperatingSystem.IsWindows())
        {
            var result = await runner.RunAsync(
                "powershell",
                [
                    "-NoProfile", "-Command",
                    "Get-PnpDevice -Class Ports -Status OK -ErrorAction SilentlyContinue "
                    + "| Select-Object -ExpandProperty FriendlyName",
                ],
                null, null, null, cancellationToken).ConfigureAwait(false);

            var ports = new List<SerialPortInfo>();
            foreach (var line in result.StandardOutput.Split('\n'))
            {
                var text = line.Trim();
                if (text.Length == 0) continue;
                var match = ComPortPattern().Match(text);
                if (!match.Success) continue;
                ports.Add(new SerialPortInfo(match.Groups[1].Value, text));
            }
            return ports;
        }

        // Linux and macOS: the tty nodes are the enumeration.
        var candidates = new List<SerialPortInfo>();
        foreach (var directory in (string[])["/dev"])
        {
            if (!Directory.Exists(directory)) continue;
            foreach (var path in Directory.EnumerateFiles(directory))
            {
                var name = Path.GetFileName(path);
                var isSerial = name.StartsWith("ttyUSB", StringComparison.Ordinal)
                    || name.StartsWith("ttyACM", StringComparison.Ordinal)
                    || name.StartsWith("cu.usb", StringComparison.Ordinal)
                    || name.StartsWith("tty.usb", StringComparison.Ordinal);
                if (isSerial) candidates.Add(new SerialPortInfo(path, name));
            }
        }
        return candidates;
    }

    [GeneratedRegex(@"\((COM\d+)\)")]
    private static partial Regex ComPortPattern();

    /// <summary>Debug probes `probe-rs` can see.</summary>
    public async Task<IReadOnlyList<string>> ListProbesAsync(
        CancellationToken cancellationToken = default)
    {
        if (!await HasToolAsync("probe-rs", cancellationToken).ConfigureAwait(false))
        {
            return [];
        }

        var result = await runner
            .RunAsync("probe-rs", ["list"], null, null, null, cancellationToken)
            .ConfigureAwait(false);

        var probes = new List<string>();
        foreach (var line in result.Combined.Split('\n'))
        {
            var text = line.Trim();
            // `probe-rs list` prints "No debug probes were found." when empty.
            if (text.Length == 0 || text.StartsWith("No ", StringComparison.Ordinal)) continue;
            probes.Add(text);
        }
        return probes;
    }

    // ── status ──────────────────────────────────────────────────────────

    /// <summary>
    /// A status line for every board, without touching any of them.
    /// </summary>
    public async Task<IReadOnlyList<DeviceStatus>> RefreshAsync(
        CancellationToken cancellationToken = default)
    {
        var ports = await ListSerialPortsAsync(cancellationToken).ConfigureAwait(false);
        var probes = await ListProbesAsync(cancellationToken).ConfigureAwait(false);

        var statuses = new List<DeviceStatus>();
        foreach (var board in BoardCatalog.All)
        {
            statuses.Add(await StatusForAsync(board, ports, probes, cancellationToken)
                .ConfigureAwait(false));
        }
        return statuses;
    }

    private async Task<DeviceStatus> StatusForAsync(
        Board board,
        IReadOnlyList<SerialPortInfo> ports,
        IReadOnlyList<string> probes,
        CancellationToken cancellationToken)
    {
        var firmware = FindFirmware(board);
        var tool = ToolFor(board.Flash);

        if (!await HasToolAsync(tool, cancellationToken).ConfigureAwait(false))
        {
            return new DeviceStatus
            {
                Board = board,
                State = DeviceState.ToolMissing,
                Detail = $"`{tool}` is not on PATH — install it to flash this board",
                Firmware = firmware,
            };
        }

        switch (board.Flash)
        {
            case FlashTool.EspFlash:
            {
                // A USB-serial bridge is all that can be seen from here. Which
                // chip is behind it needs a handshake, which is `Identify`.
                var bridges = ports
                    .Where(p => LooksLikeUsbSerial(p.Description))
                    .ToList();
                if (bridges.Count == 0)
                {
                    return new DeviceStatus
                    {
                        Board = board,
                        State = DeviceState.NotConnected,
                        Detail = "no USB-serial bridge found",
                        Firmware = firmware,
                    };
                }
                return new DeviceStatus
                {
                    Board = board,
                    State = DeviceState.Ambiguous,
                    Endpoint = bridges[0].PortName,
                    Detail = bridges.Count == 1
                        ? $"{bridges[0].Description} — run Identify to confirm the chip"
                        : $"{bridges.Count} bridges; pick one and run Identify",
                    Firmware = firmware,
                };
            }

            case FlashTool.ProbeRs:
            {
                if (probes.Count == 0)
                {
                    return new DeviceStatus
                    {
                        Board = board,
                        State = DeviceState.NotConnected,
                        Detail = "no debug probe attached",
                        Firmware = firmware,
                    };
                }
                // A probe says nothing about what is on the other end of SWD.
                return new DeviceStatus
                {
                    Board = board,
                    State = DeviceState.Ambiguous,
                    Endpoint = probes[0],
                    Detail = $"{probes[0]} — a probe is attached; Identify reads the target",
                    Firmware = firmware,
                };
            }

            case FlashTool.DfuUtil:
            {
                var result = await runner
                    .RunAsync("dfu-util", ["-l"], null, null, null, cancellationToken)
                    .ConfigureAwait(false);
                var found = result.Combined.Contains("Found DFU", StringComparison.OrdinalIgnoreCase);
                return new DeviceStatus
                {
                    Board = board,
                    State = found ? DeviceState.Connected : DeviceState.NotConnected,
                    Endpoint = found ? "USB DFU" : null,
                    Detail = found
                        ? "in DFU mode and ready to flash"
                        : "not in DFU mode — hold BOOT and reset to enter it",
                    Firmware = firmware,
                };
            }

            case FlashTool.Uf2:
            {
                var volume = FindUf2Volume();
                return new DeviceStatus
                {
                    Board = board,
                    State = volume is null ? DeviceState.NotConnected : DeviceState.Connected,
                    Endpoint = volume,
                    Detail = volume is null
                        ? "no UF2 volume mounted — hold BOOTSEL while plugging it in"
                        : $"UF2 volume at {volume}; copying the image flashes it",
                    Firmware = firmware,
                };
            }

            default:
                return new DeviceStatus
                {
                    Board = board,
                    State = DeviceState.NotConnected,
                    Detail = "no detection method for this transport",
                    Firmware = firmware,
                };
        }
    }

    /// <summary>
    /// Whether a port description looks like a USB-serial bridge.
    ///
    /// Deliberately a heuristic over the common bridge chips, and deliberately
    /// only ever used to produce <see cref="DeviceState.Ambiguous"/>.
    /// </summary>
    private static bool LooksLikeUsbSerial(string description)
    {
        foreach (var marker in (string[])["CH340", "CP210", "FT232", "USB-SERIAL", "USB Serial", "UART", "ACM"])
        {
            if (description.Contains(marker, StringComparison.OrdinalIgnoreCase)) return true;
        }
        return false;
    }

    /// <summary>A mounted volume that looks like an RP2040 in BOOTSEL mode.</summary>
    private static string? FindUf2Volume()
    {
        foreach (var drive in DriveInfo.GetDrives())
        {
            try
            {
                if (!drive.IsReady) continue;
                // The bootloader's volume label is fixed, and INFO_UF2.TXT is
                // the file the specification requires it to expose.
                if (drive.VolumeLabel.Contains("RPI-RP2", StringComparison.OrdinalIgnoreCase)
                    || File.Exists(Path.Combine(drive.RootDirectory.FullName, "INFO_UF2.TXT")))
                {
                    return drive.RootDirectory.FullName;
                }
            }
            catch
            {
                // A drive that vanished between enumeration and inspection is
                // not an error; it is a removable drive behaving normally.
            }
        }
        return null;
    }

    // ── identity ────────────────────────────────────────────────────────

    /// <summary>
    /// Asks the chip what it is.
    ///
    /// This is the one operation that talks to the board, and it is separate
    /// from <see cref="RefreshAsync"/> for that reason: `espflash board-info`
    /// resets the part to enter its bootloader, which would be a surprising
    /// side effect of opening a status panel.
    /// </summary>
    public async Task<ProcessResult> IdentifyAsync(
        Board board,
        string? endpoint,
        Action<string> log,
        CancellationToken cancellationToken = default)
    {
        switch (board.Flash)
        {
            case FlashTool.EspFlash:
            {
                List<string> arguments = ["board-info"];
                if (!string.IsNullOrWhiteSpace(endpoint))
                {
                    arguments.Add("--port");
                    arguments.Add(endpoint);
                }
                log($"$ espflash {string.Join(' ', arguments)}");
                log("(this resets the board into its bootloader)");
                return await runner
                    .RunAsync("espflash", arguments, null, log, log, cancellationToken)
                    .ConfigureAwait(false);
            }

            case FlashTool.ProbeRs:
            {
                List<string> arguments = ["info"];
                if (board.ProbeChip is { } chip)
                {
                    arguments.Add("--chip");
                    arguments.Add(chip);
                }
                log($"$ probe-rs {string.Join(' ', arguments)}");
                return await runner
                    .RunAsync("probe-rs", arguments, null, log, log, cancellationToken)
                    .ConfigureAwait(false);
            }

            case FlashTool.DfuUtil:
                log("$ dfu-util -l");
                return await runner
                    .RunAsync("dfu-util", ["-l"], null, log, log, cancellationToken)
                    .ConfigureAwait(false);

            default:
                log($"{board.Name} is flashed by copying a file; there is nothing to interrogate.");
                return new ProcessResult(0, "", "", TimeSpan.Zero);
        }
    }

    // ── building and flashing ───────────────────────────────────────────

    /// <summary>The firmware crate directory for a board, if the repo is known.</summary>
    public string? FirmwareCrate(Board board) =>
        RepositoryRoot is null
            ? null
            : Path.Combine(RepositoryRoot, "embedded", board.FirmwareDirectory);

    /// <summary>The built image on disk, if there is one.</summary>
    public FirmwareInfo? FindFirmware(Board board)
    {
        var crate = FirmwareCrate(board);
        if (crate is null) return null;

        var directory = Path.Combine(crate, "target", board.Target, "release");
        if (!Directory.Exists(directory)) return null;

        // The binary has no extension on these targets, so pick the newest
        // extensionless file rather than guessing the crate's binary name.
        var candidates = Directory.EnumerateFiles(directory)
            .Where(f => Path.GetExtension(f).Length == 0)
            .Select(f => new FileInfo(f))
            .OrderByDescending(f => f.LastWriteTimeUtc)
            .ToList();

        var image = candidates.FirstOrDefault();
        return image is null
            ? null
            : new FirmwareInfo(image.FullName, image.Length, image.LastWriteTimeUtc);
    }

    /// <summary>
    /// Builds a board's firmware, optionally embedding a specific assembly.
    ///
    /// <paramref name="applicationDll"/> is passed through the
    /// <c>RUSTCLR_APP</c> environment variable, which each firmware's
    /// <c>build.rs</c> turns into the path `include_bytes!` reads. That is what
    /// makes "deploy this program to the board" a real operation rather than
    /// editing the firmware crate by hand: the app is *in* the image, because
    /// these boards have no filesystem to put it in.
    /// </summary>
    public async Task<ProcessResult> BuildFirmwareAsync(
        Board board,
        string? applicationDll,
        Action<string> log,
        CancellationToken cancellationToken = default)
    {
        var crate = FirmwareCrate(board);
        if (crate is null || !Directory.Exists(crate))
        {
            log($"Firmware crate not found for {board.Name}.");
            log("Open the RustNetRuntime repository as the project, or set the path in Settings.");
            return new ProcessResult(-1, "", "no firmware crate", TimeSpan.Zero);
        }

        List<string> arguments = [];
        if (board.Toolchain is { } toolchain) arguments.Add(toolchain);
        arguments.AddRange(["build", "--release"]);
        if (board.CargoFeature is { } feature)
        {
            arguments.AddRange(["--no-default-features", "--features", feature]);
        }
        arguments.AddRange(["--target", board.Target]);
        arguments.AddRange(board.ExtraCargoArgs);

        if (applicationDll is not null)
        {
            log($"embedding {Path.GetFileName(applicationDll)} into the image");
        }
        log($"$ cargo {string.Join(' ', arguments)}");

        var environment = applicationDll is null
            ? null
            : new Dictionary<string, string> { ["RUSTCLR_APP"] = applicationDll };

        return await runner
            .RunAsync("cargo", arguments, crate, log, log, cancellationToken, environment)
            .ConfigureAwait(false);
    }

    /// <summary>
    /// Builds and flashes a board, embedding an assembly if one is given.
    /// </summary>
    public async Task<ProcessResult> FlashAsync(
        Board board,
        string? applicationDll,
        string? endpoint,
        Action<string> log,
        CancellationToken cancellationToken = default)
    {
        if (applicationDll is not null && board.Tier == BclTier.None)
        {
            // Flashing would work. The program would not run, and the board
            // would say so on its console — but saying it here saves a build
            // and a flash cycle.
            var (minimal, _) = BoardCatalog.TierBytes;
            log($"{board.Name} has {board.HeapBytes / 1024} KB of heap; the interpreter needs");
            log($"{minimal:N0} bytes to load even the reduced binding set. The image will be");
            log("flashed, but it will report the shortfall rather than run the program.");
        }

        var build = await BuildFirmwareAsync(board, applicationDll, log, cancellationToken)
            .ConfigureAwait(false);
        if (!build.Succeeded)
        {
            log("Firmware build failed; nothing was written to the board.");
            return build;
        }

        var firmware = FindFirmware(board);
        if (firmware is null)
        {
            log("Build reported success but no image was found.");
            return new ProcessResult(-1, "", "no image", TimeSpan.Zero);
        }
        log($"image: {firmware.Path} ({firmware.SizeLabel})");

        var tool = ToolFor(board.Flash);
        if (!await HasToolAsync(tool, cancellationToken).ConfigureAwait(false))
        {
            log($"`{tool}` is not on PATH, so the image was built but not flashed.");
            return new ProcessResult(-1, "", $"{tool} missing", TimeSpan.Zero);
        }

        switch (board.Flash)
        {
            case FlashTool.EspFlash:
            {
                List<string> arguments = ["flash", "--chip", board.ProbeChip!];
                if (!string.IsNullOrWhiteSpace(endpoint))
                {
                    arguments.AddRange(["--port", endpoint]);
                }
                arguments.AddRange(["--non-interactive", firmware.Path]);
                log($"$ espflash {string.Join(' ', arguments)}");
                return await runner
                    .RunAsync("espflash", arguments, null, log, log, cancellationToken)
                    .ConfigureAwait(false);
            }

            case FlashTool.ProbeRs:
            {
                List<string> arguments = ["download"];
                if (board.ProbeChip is { } chip) arguments.AddRange(["--chip", chip]);
                arguments.Add(firmware.Path);
                log($"$ probe-rs {string.Join(' ', arguments)}");
                return await runner
                    .RunAsync("probe-rs", arguments, null, log, log, cancellationToken)
                    .ConfigureAwait(false);
            }

            case FlashTool.DfuUtil:
            {
                // The Meadow's DFU flow needs a raw binary at a load address,
                // not an ELF. Converting one is `objcopy`'s job and is done by
                // the crate's own README steps, so this stops at the boundary
                // rather than half-doing it.
                log("Flashing the Meadow F7 over DFU replaces the contents of internal flash.");
                log("That is not reversible without a backup, so it is left to the documented");
                log($"steps in embedded/{board.FirmwareDirectory}/README.md rather than done from here.");
                return new ProcessResult(-1, "", "dfu flow is manual by design", TimeSpan.Zero);
            }

            case FlashTool.Uf2:
            {
                var volume = FindUf2Volume();
                if (volume is null)
                {
                    log("No UF2 volume is mounted. Hold BOOTSEL while plugging the board in.");
                    return new ProcessResult(-1, "", "no uf2 volume", TimeSpan.Zero);
                }
                var uf2 = Path.ChangeExtension(firmware.Path, ".uf2");
                if (!File.Exists(uf2))
                {
                    log($"No .uf2 beside the image. Run tools/elf2uf2.py in embedded/{board.FirmwareDirectory} first.");
                    return new ProcessResult(-1, "", "no uf2", TimeSpan.Zero);
                }
                var destination = Path.Combine(volume, Path.GetFileName(uf2));
                log($"copying {Path.GetFileName(uf2)} to {volume}");
                File.Copy(uf2, destination, overwrite: true);
                log("Copied. The board reboots into the new firmware on its own.");
                return new ProcessResult(0, "", "", TimeSpan.Zero);
            }

            default:
                log($"No flashing method is wired up for {board.Name}.");
                return new ProcessResult(-1, "", "unsupported", TimeSpan.Zero);
        }
    }

    /// <summary>
    /// Opens a serial monitor on the board's console.
    /// </summary>
    public async Task<ProcessResult> MonitorAsync(
        Board board,
        string? endpoint,
        Action<string> log,
        CancellationToken cancellationToken = default)
    {
        if (board.Flash != FlashTool.EspFlash)
        {
            log($"{board.Name}'s console is {board.Console}.");
            log("Attach a terminal to it at 115200 baud; there is no monitor wired up here.");
            return new ProcessResult(-1, "", "no monitor", TimeSpan.Zero);
        }

        List<string> arguments = ["monitor", "--non-interactive"];
        if (board.ProbeChip is { } chip) arguments.AddRange(["--chip", chip]);
        if (!string.IsNullOrWhiteSpace(endpoint)) arguments.AddRange(["--port", endpoint]);
        log($"$ espflash {string.Join(' ', arguments)}");
        return await runner
            .RunAsync("espflash", arguments, null, log, log, cancellationToken)
            .ConfigureAwait(false);
    }
}
