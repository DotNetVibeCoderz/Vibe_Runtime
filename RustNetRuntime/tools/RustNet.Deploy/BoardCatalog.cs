namespace RustNet.Deploy;

/// <summary>
/// How a board's firmware is flashed once it is built.
/// </summary>
public enum FlashKind
{
    /// <summary>Nothing to flash — the host virtual device is just run.</summary>
    None,

    /// <summary>Espressif's <c>espflash</c>, over the board's USB-serial bridge.</summary>
    Espflash,

    /// <summary>Kendryte's <c>kflash</c>, which wants a raw binary rather than an ELF.</summary>
    Kflash,

    /// <summary><c>probe-rs</c> over SWD — needs a debug probe, on-board or external.</summary>
    ProbeRs,

    /// <summary><c>dfu-util</c>, for boards whose ROM speaks USB DFU.</summary>
    Dfu,

    /// <summary>A UF2 copied onto the mass-storage device the ROM bootloader exposes.</summary>
    Uf2,
}

/// <summary>
/// One step of getting firmware onto a board.
/// </summary>
/// <remarks>
/// Steps are data rather than code so the same plan can be run by the CLI, run
/// by the Workbench with its output streamed into a panel, or printed for
/// someone who would rather type it themselves.
/// </remarks>
public abstract record FlashStep(string Description);

/// <summary>Run a program and wait for it.</summary>
public sealed record RunStep(string Description, string Exe, string Args, string WorkingDirectory)
    : FlashStep(Description);

/// <summary>
/// Ask a running RustNet device to reboot into its ROM bootloader.
/// </summary>
/// <remarks>
/// Only the RP2040 port answers this, and only because its firmware can call
/// the boot ROM's <c>reset_to_usb_boot</c>. It turns reflashing from "unplug
/// the board, hold BOOTSEL, plug it back in" into something a tool can do on
/// its own — which matters most when the board is not on the desk.
/// </remarks>
public sealed record EnterBootloaderStep(string Description, string DeviceSpec)
    : FlashStep(Description);

/// <summary>
/// Copy a UF2 onto the removable volume a ROM bootloader exposes.
/// </summary>
public sealed record CopyToVolumeStep(string Description, string SourcePath, string VolumeLabel)
    : FlashStep(Description);

/// <summary>
/// A board this repository has firmware for: where it is built, what comes
/// out, and how that gets onto the silicon.
/// </summary>
public sealed record BoardRecipe
{
    /// <summary>Short name used on the command line and in the pickers.</summary>
    public required string Id { get; init; }

    /// <summary>What the board is called on its own box.</summary>
    public required string Name { get; init; }

    /// <summary>
    /// The chip family an application must be signed for to run here. A device
    /// refuses an image built for another chip, so the two pickers have to
    /// agree.
    /// </summary>
    public required string Chip { get; init; }

    /// <summary>
    /// The crate directory, relative to the repository root. The bare-metal
    /// ports are standalone workspaces — they need their own toolchain and are
    /// excluded from <c>cargo test --workspace</c> — so the build runs there
    /// rather than at the root.
    /// </summary>
    public required string WorkspaceDir { get; init; }

    /// <summary>Arguments to <c>cargo</c>, without the word <c>cargo</c>.</summary>
    public required string BuildArgs { get; init; }

    /// <summary>The binary cargo produces, before any objcopy.</summary>
    public required string BinaryName { get; init; }

    public required FlashKind Flash { get; init; }

    /// <summary>
    /// What has to be installed for <see cref="Flash"/> to work. Reported up
    /// front, because a missing vendor tool otherwise surfaces as a bare
    /// "file not found" from the process launcher.
    /// </summary>
    public required string Requires { get; init; }

    /// <summary>One line on what is peculiar about this board.</summary>
    public required string Note { get; init; }

    /// <summary>
    /// Environment the build needs, on top of what the shell already has.
    /// </summary>
    /// <remarks>
    /// The ESP-IDF build system takes the target SoC from <c>MCU</c>, and a
    /// workspace's <c>.cargo/config.toml</c> sets a default there. Cargo's
    /// <c>[env]</c> does not override a variable the environment already
    /// carries, so setting it here is how one workspace builds for two chips.
    /// </remarks>
    public IReadOnlyDictionary<string, string> Env { get; init; } =
        new Dictionary<string, string>();

    /// <summary>True when the board needs a serial port named to flash it.</summary>
    public bool NeedsPort => Flash is FlashKind.Espflash or FlashKind.Kflash;
}

/// <summary>
/// Every board this repository can build firmware for.
/// </summary>
/// <remarks>
/// One table, shared by <c>rustnet firmware</c> and the Workbench, because two
/// copies of a recipe drift and the one you are not looking at is the one that
/// is wrong. Everything here is derived from what has actually been run against
/// hardware — see each port's README.
/// </remarks>
public static class BoardCatalog
{
    public static IReadOnlyList<BoardRecipe> All { get; } =
    [
        new BoardRecipe
        {
            Id = "host",
            Name = "Virtual device (this machine)",
            Chip = "host-sim",
            WorkspaceDir = "",
            BuildArgs = "build -p rustnet-firmware",
            BinaryName = "rustclr-firmware",
            Flash = FlashKind.None,
            Requires = "nothing beyond a Rust toolchain",
            Note = "Not silicon: a simulator that speaks the same protocol. Run it, do not flash it.",
        },
        new BoardRecipe
        {
            Id = "esp32",
            Name = "ESP32 DevKit / WROOM",
            Chip = "esp32",
            WorkspaceDir = "embedded/esp32-demo",
            BuildArgs = "build --release",
            BinaryName = "rustclr-esp32",
            Flash = FlashKind.Espflash,
            Requires = "the `esp` Rust toolchain (espup) and espflash",
            Note = "Flashed with the custom partition table, or the FAT storage partition does not exist "
                 + "and apps, provisioning and autostart do not survive a reboot.",
        },
        new BoardRecipe
        {
            Id = "m5tough",
            Name = "M5Stack Tough",
            Chip = "esp32",
            WorkspaceDir = "embedded/esp32-demo",
            BuildArgs = "build --release --features board-m5tough",
            BinaryName = "rustclr-esp32",
            Flash = FlashKind.Espflash,
            Requires = "the `esp` Rust toolchain (espup) and espflash",
            Note = "AXP192 powers the LCD rails before the ILI9342C panel is driven; the 320x240 "
                 + "framebuffer is 150 KB and needs PSRAM.",
        },
        new BoardRecipe
        {
            Id = "m5core2",
            Name = "M5Stack Core2",
            Chip = "esp32",
            WorkspaceDir = "embedded/esp32-demo",
            BuildArgs = "build --release --features board-m5core2",
            BinaryName = "rustclr-esp32",
            Flash = FlashKind.Espflash,
            Requires = "the `esp` Rust toolchain (espup) and espflash",
            Note = "Same ILI9342C panel and AXP192 as the Tough, but LDO3 drives the vibration "
                 + "motor here — powering it the Tough's way leaves the board buzzing forever.",
        },
        new BoardRecipe
        {
            Id = "esp32c3",
            Name = "ESP32-C3 (Seeed XIAO and similar)",
            Chip = "esp32c3",
            WorkspaceDir = "embedded/esp32-demo",
            BuildArgs = "build --release --no-default-features --features chip-esp32c3 "
                      + "--target riscv32imc-esp-espidf",
            BinaryName = "rustclr-esp32",
            Flash = FlashKind.Espflash,
            Env = new Dictionary<string, string> { ["MCU"] = "esp32c3" },
            Requires = "the `esp` Rust toolchain and espflash",
            Note = "RNDP runs over the SoC's own USB Serial/JTAG, not UART0 — these boards wire "
                 + "the USB socket straight to the controller inside the chip.",
        },
        new BoardRecipe
        {
            Id = "k210",
            Name = "Sipeed Maix Go (K210)",
            Chip = "k210",
            WorkspaceDir = "embedded/k210",
            BuildArgs = "build --release",
            BinaryName = "rustclr-k210",
            Flash = FlashKind.Kflash,
            Requires = "cargo-binutils (rust-objcopy) and kflash",
            Note = "kflash wants a raw binary, so the ELF is objcopy'd first. The openec bridge "
                 + "asserts reset whenever DTR and RTS are both set.",
        },
        new BoardRecipe
        {
            Id = "stm32",
            Name = "STM32F401 / Nucleo-F401RE",
            Chip = "stm32",
            WorkspaceDir = "embedded/stm32f4",
            BuildArgs = "build --release",
            BinaryName = "rustclr-stm32f4",
            Flash = FlashKind.ProbeRs,
            Requires = "probe-rs-tools, and the board's on-board ST-Link",
            Note = "`probe-rs run` waits for RTT this binary does not emit, so it is downloaded "
                 + "and reset instead.",
        },
        new BoardRecipe
        {
            Id = "netduino3",
            Name = "Netduino 3 WiFi",
            Chip = "stm32",
            WorkspaceDir = "embedded/stm32f4",
            BuildArgs = "build --release",
            BinaryName = "rustclr-stm32f4",
            Flash = FlashKind.Dfu,
            Requires = "cargo-binutils (rust-objcopy) and dfu-util",
            Note = "No debug probe: the STM32 ROM's USB DFU is the way in. Hold the boot button "
                 + "while plugging it in so 0483:df11 enumerates.",
        },
        new BoardRecipe
        {
            Id = "meadow-f7",
            Name = "Wilderness Labs Meadow F7 Micro",
            Chip = "stm32",
            WorkspaceDir = "embedded/meadow-f7",
            BuildArgs = "build --release",
            BinaryName = "rustclr-meadow-f7",
            Flash = FlashKind.Dfu,
            Requires = "cargo-binutils (rust-objcopy) and dfu-util",
            Note = "Flashing this REPLACES Meadow OS in internal flash — DFU is the only way in "
                 + "without a probe. Reversible with Wilderness Labs' own `meadow` CLI. Hold BOOT "
                 + "and tap RST so 0483:df11 enumerates — needed only the FIRST time, since a "
                 + "RustNet image can put itself into DFU when --device names its port. Verified "
                 + "on hardware: RNDP over the board's own USB, a serial console on D0/D1, 32 MB "
                 + "of QSPI storage, and WiFi via the ESP32 coprocessor running ESP-AT.",
        },
        new BoardRecipe
        {
            Id = "pico",
            Name = "Raspberry Pi Pico (RP2040)",
            Chip = "rp2040",
            WorkspaceDir = "embedded/rp2040",
            BuildArgs = "build --release",
            BinaryName = "rustclr-rp2040",
            Flash = FlashKind.Uf2,
            Requires = "python (for the UF2 packer); no vendor tool and no probe",
            Note = "The only board here that can put itself into its bootloader — a running RustNet "
                 + "Pico is asked over RNDP, so nothing has to touch BOOTSEL.",
        },
    ];

    public static BoardRecipe? Find(string id) =>
        All.FirstOrDefault(b => string.Equals(b.Id, id, StringComparison.OrdinalIgnoreCase));

    /// <summary>
    /// Where a workspace puts its build output, read from its own
    /// <c>.cargo/config.toml</c> rather than assumed.
    /// </summary>
    /// <remarks>
    /// The ESP32 workspace redirects its target directory off the repository
    /// (esp-idf-sys refuses long paths on Windows) and every bare-metal port
    /// sets a default target triple. Both live in that file, so reading it is
    /// how the artifact path stays right when someone changes it.
    /// </remarks>
    public static (string TargetDir, string? Triple) ResolveTarget(string repoRoot, BoardRecipe board)
    {
        string ws = string.IsNullOrEmpty(board.WorkspaceDir)
            ? repoRoot
            : Path.Combine(repoRoot, board.WorkspaceDir);
        string config = Path.Combine(ws, ".cargo", "config.toml");

        string targetDir = Path.Combine(ws, "target");
        string? triple = null;

        if (File.Exists(config))
        {
            bool inBuild = false;
            foreach (string raw in File.ReadAllLines(config))
            {
                string line = raw.Trim();
                if (line.StartsWith('['))
                {
                    // Only the `[build]` table's keys matter; `target` appears
                    // again under `[target.<triple>]` headers meaning something
                    // else entirely.
                    inBuild = line == "[build]";
                    continue;
                }
                if (!inBuild)
                {
                    continue;
                }
                if (TomlValue(line, "target-dir") is { } dir)
                {
                    targetDir = Path.IsPathRooted(dir) ? dir : Path.Combine(ws, dir);
                }
                else if (TomlValue(line, "target") is { } t)
                {
                    triple = t;
                }
            }
        }

        // A build argument wins over the config: `--target x` is how a second
        // chip is built out of one workspace.
        int at = board.BuildArgs.IndexOf("--target ", StringComparison.Ordinal);
        if (at >= 0)
        {
            triple = board.BuildArgs[(at + 9)..].Split(' ')[0];
        }

        return (targetDir, triple);
    }

    /// <summary>
    /// Pull <c>key = "value"</c> out of one line, or null if it is not that key.
    /// </summary>
    private static string? TomlValue(string line, string key)
    {
        if (!line.StartsWith(key, StringComparison.Ordinal))
        {
            return null;
        }
        string rest = line[key.Length..].TrimStart();
        if (!rest.StartsWith('='))
        {
            // `target-dir` starts with `target`, so a prefix match alone would
            // read the wrong key.
            return null;
        }
        rest = rest[1..].Trim();
        int hash = rest.IndexOf('#');
        if (hash >= 0)
        {
            rest = rest[..hash].Trim();
        }
        return rest.Trim('"', '\'');
    }

    /// <summary>The ELF cargo produces for this board.</summary>
    public static string ArtifactPath(string repoRoot, BoardRecipe board)
    {
        var (targetDir, triple) = ResolveTarget(repoRoot, board);
        string profile = board.BuildArgs.Contains("--release", StringComparison.Ordinal) ? "release" : "debug";
        string name = board.BinaryName;
        if (board.Flash == FlashKind.None && OperatingSystem.IsWindows())
        {
            name += ".exe";
        }
        return triple is null
            ? Path.Combine(targetDir, profile, name)
            : Path.Combine(targetDir, triple, profile, name);
    }

    /// <summary>Where this board's working directory is.</summary>
    public static string WorkspacePath(string repoRoot, BoardRecipe board) =>
        string.IsNullOrEmpty(board.WorkspaceDir) ? repoRoot : Path.Combine(repoRoot, board.WorkspaceDir);

    /// <summary>
    /// The steps that put an already-built image onto the board.
    /// </summary>
    /// <param name="port">
    /// The serial port, for boards flashed over one. Ignored otherwise.
    /// </param>
    /// <param name="deviceSpec">
    /// A running device to ask into its bootloader first, where the port
    /// supports it. Null skips that step and the board is expected to already
    /// be in its bootloader.
    /// </param>
    public static IReadOnlyList<FlashStep> FlashPlan(
        string repoRoot,
        BoardRecipe board,
        string? port,
        string? deviceSpec = null)
    {
        string ws = WorkspacePath(repoRoot, board);
        string elf = ArtifactPath(repoRoot, board);
        var steps = new List<FlashStep>();

        switch (board.Flash)
        {
            case FlashKind.None:
                break;

            case FlashKind.Espflash:
                // The partition table is not optional: without it there is no
                // FAT storage partition, and everything the device is supposed
                // to remember is lost on the next power cycle.
                steps.Add(new RunStep(
                    $"espflash -> {port}",
                    "espflash",
                    $"flash \"{elf}\" --partition-table partitions.csv --port {port}",
                    ws));
                break;

            case FlashKind.Kflash:
                steps.Add(new RunStep(
                    "rust-objcopy -> fw.bin",
                    "rust-objcopy",
                    $"-O binary \"{elf}\" fw.bin",
                    ws));
                steps.Add(new RunStep(
                    $"kflash -> {port}",
                    "kflash",
                    $"-p {port} -b 1500000 fw.bin",
                    ws));
                break;

            case FlashKind.ProbeRs:
                steps.Add(new RunStep(
                    "probe-rs download",
                    "probe-rs",
                    $"download --chip STM32F401RE \"{elf}\"",
                    ws));
                steps.Add(new RunStep(
                    "probe-rs reset",
                    "probe-rs",
                    "reset --chip STM32F401RE",
                    ws));
                break;

            case FlashKind.Dfu:
                steps.Add(new RunStep(
                    "rust-objcopy -> fw.bin",
                    "rust-objcopy",
                    $"-O binary \"{elf}\" fw.bin",
                    ws));
                // A RustNet image already on the board can put itself into the
                // ROM's DFU bootloader, which is the difference between a
                // one-command reflash and holding a boot pin through a power
                // cycle. Only when a device was named: the Netduino shares
                // this path and has no such firmware on it yet.
                if (deviceSpec is not null)
                {
                    steps.Add(new EnterBootloaderStep(
                        "ask the running device into DFU", deviceSpec));
                }
                steps.Add(new RunStep(
                    "dfu-util -> 0x08000000",
                    "dfu-util",
                    "-d 0483:df11 -a 0 -s 0x08000000:leave -D fw.bin",
                    ws));
                break;

            case FlashKind.Uf2:
                // Next to the ELF rather than in the crate: it is a build
                // artifact, and the target directory is already ignored.
                string uf2 = Path.Combine(Path.GetDirectoryName(elf)!, "rustnet-pico.uf2");
                steps.Add(new RunStep(
                    "pack UF2",
                    "python",
                    $"tools/elf2uf2.py \"{elf}\" \"{uf2}\"",
                    ws));
                if (deviceSpec is not null)
                {
                    steps.Add(new EnterBootloaderStep(
                        "ask the running device into its bootloader", deviceSpec));
                }
                steps.Add(new CopyToVolumeStep("copy UF2 to the board", uf2, "RPI-RP2"));
                break;
        }

        return steps;
    }
}
