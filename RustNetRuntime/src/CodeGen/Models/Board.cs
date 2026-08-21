namespace CodeGen.Models;

/// <summary>How an image gets onto a board.</summary>
public enum FlashTool
{
    /// <summary>`espflash`, over a USB-serial bridge. ESP32 family.</summary>
    EspFlash,

    /// <summary>`probe-rs`, over SWD. Needs a debug probe.</summary>
    ProbeRs,

    /// <summary>`dfu-util`, over USB DFU. No probe, but the board must be in DFU mode.</summary>
    DfuUtil,

    /// <summary>A UF2 file copied to a mass-storage volume the bootloader exposes.</summary>
    Uf2,
}

/// <summary>
/// How much of RustBCL a board's memory can hold.
///
/// Mirrors <c>Tier</c> in <c>embedded/demo-common</c>, and the byte figures are
/// the same measured ones. Duplicated rather than shared because there is no
/// way for a C# IDE to read a Rust constant, and a stale copy here would
/// mislead about what a board can run — so
/// <see cref="BoardCatalog.TierBytes"/> names the source of truth.
/// </summary>
public enum BclTier
{
    /// <summary>Not enough RAM to load the runtime at all.</summary>
    None,

    /// <summary>Console, strings and maths. No LINQ, collections or reflection.</summary>
    Minimal,

    /// <summary>Every native binding RustBCL has.</summary>
    Full,
}

/// <summary>
/// One board this repository ships a firmware for.
///
/// The facts here are the same ones in <c>embedded/*/README.md</c> and
/// <c>docs/limitations.md</c>. They are what the Deploy and Device Status
/// features need in order to say something specific rather than generic.
/// </summary>
public sealed record Board
{
    public required string Id { get; init; }
    public required string Name { get; init; }
    public required string Chip { get; init; }
    public required string Core { get; init; }

    /// <summary>Rust target triple the firmware builds for.</summary>
    public required string Target { get; init; }

    /// <summary>Directory under <c>embedded/</c> holding the firmware crate.</summary>
    public required string FirmwareDirectory { get; init; }

    /// <summary>Cargo feature selecting this board, when the crate serves several.</summary>
    public string? CargoFeature { get; init; }

    /// <summary>Extra cargo arguments — the Xtensa toolchain needs `build-std`.</summary>
    public IReadOnlyList<string> ExtraCargoArgs { get; init; } = [];

    /// <summary>Cargo toolchain override, e.g. <c>+esp</c>.</summary>
    public string? Toolchain { get; init; }

    public required FlashTool Flash { get; init; }

    /// <summary>Chip name the flashing tool wants, when it wants one.</summary>
    public string? ProbeChip { get; init; }

    /// <summary>Total RAM in bytes, for the report.</summary>
    public required int RamBytes { get; init; }

    /// <summary>Bytes the firmware hands the allocator.</summary>
    public required int HeapBytes { get; init; }

    public required BclTier Tier { get; init; }

    /// <summary>
    /// Whether this exact board has been run, on hardware, since the
    /// interpreter landed. Kept honest deliberately: the UI says "never
    /// flashed" rather than implying verification that did not happen.
    /// </summary>
    public bool VerifiedOnHardware { get; init; }

    /// <summary>How a host sees the board's console, for the status panel.</summary>
    public required string Console { get; init; }

    public string TierLabel => Tier switch
    {
        BclTier.Full => "full RustBCL",
        BclTier.Minimal => "minimal RustBCL",
        BclTier.None => "no interpreter",
        _ => Tier.ToString(),
    };

    /// <summary>One line for a list: what it is and what it can run.</summary>
    public string Summary => $"{Chip} · {Core} · {RamBytes / 1024} KB RAM · {TierLabel}";
}

/// <summary>
/// The boards <c>embedded/</c> has firmware for.
///
/// This is a mirror of <c>tests/firmware.sh</c>. If a board is added there and
/// not here, Deploy simply will not offer it — which is a visible gap rather
/// than a wrong answer, and is why the two lists are checked against each other
/// by <c>BoardCatalogTests</c>.
/// </summary>
public static class BoardCatalog
{
    /// <summary>
    /// Peak bytes needed to load the runtime at each tier.
    ///
    /// Measured with a counting allocator around the loader, RustBCL's
    /// registration and one program run; see <c>docs/limitations.md</c> and
    /// <c>Tier::for_budget</c> in <c>embedded/demo-common</c>, which is the
    /// source of truth. Shown in the UI so a board's tier reads as arithmetic
    /// rather than as an opinion.
    /// </summary>
    public static (int Minimal, int Full) TierBytes => (192_045, 260_702);

    public static IReadOnlyList<Board> All { get; } =
    [
        new()
        {
            Id = "esp32c3",
            Name = "ESP32-C3",
            Chip = "ESP32-C3",
            Core = "RISC-V 32",
            Target = "riscv32imc-unknown-none-elf",
            FirmwareDirectory = "esp32-demo",
            CargoFeature = "esp32c3",
            Flash = FlashTool.EspFlash,
            ProbeChip = "esp32c3",
            RamBytes = 400 * 1024,
            HeapBytes = 288 * 1024,
            Tier = BclTier.Full,
            VerifiedOnHardware = true,
            Console = "USB-serial bridge, 115200 baud",
        },
        new()
        {
            Id = "esp32",
            Name = "ESP32-WROOM-32",
            Chip = "ESP32",
            Core = "Xtensa LX6",
            Target = "xtensa-esp32-none-elf",
            FirmwareDirectory = "esp32-demo",
            CargoFeature = "esp32",
            Toolchain = "+esp",
            ExtraCargoArgs = ["-Z", "build-std=core,alloc"],
            Flash = FlashTool.EspFlash,
            ProbeChip = "esp32",
            RamBytes = 520 * 1024,
            // 176 KB of dram_seg plus 96 KB of the reclaimed second bank.
            HeapBytes = 272 * 1024,
            Tier = BclTier.Full,
            Console = "UART0 via USB-serial bridge, 115200 baud",
        },
        new()
        {
            Id = "meadow-f7",
            Name = "Meadow F7 Micro",
            Chip = "STM32F777",
            Core = "Arm Cortex-M7",
            Target = "thumbv7em-none-eabihf",
            FirmwareDirectory = "meadow-f7",
            Flash = FlashTool.DfuUtil,
            RamBytes = 384 * 1024,
            HeapBytes = 288 * 1024,
            Tier = BclTier.Full,
            Console = "USB CDC on the board's own socket",
        },
        new()
        {
            Id = "k210",
            Name = "Sipeed Maix Go",
            Chip = "K210",
            Core = "RISC-V 64",
            Target = "riscv64gc-unknown-none-elf",
            FirmwareDirectory = "k210",
            Flash = FlashTool.ProbeRs,
            RamBytes = 6 * 1024 * 1024,
            HeapBytes = 1024 * 1024,
            Tier = BclTier.Full,
            Console = "UARTHS, 115200 baud",
        },
        new()
        {
            Id = "netduino3-f427vi",
            Name = "Netduino 3 WiFi",
            Chip = "STM32F427VI",
            Core = "Arm Cortex-M4F",
            Target = "thumbv7em-none-eabihf",
            FirmwareDirectory = "stm32f4",
            CargoFeature = "netduino3-f427vi",
            Flash = FlashTool.ProbeRs,
            ProbeChip = "STM32F427VITx",
            RamBytes = 256 * 1024,
            HeapBytes = 192 * 1024,
            Tier = BclTier.Minimal,
            Console = "UART7 on PE8 — needs a USB-serial adapter",
        },
        new()
        {
            Id = "rp2040",
            Name = "Raspberry Pi Pico",
            Chip = "RP2040",
            Core = "Arm Cortex-M0+",
            Target = "thumbv6m-none-eabi",
            FirmwareDirectory = "rp2040",
            Flash = FlashTool.Uf2,
            RamBytes = 256 * 1024,
            HeapBytes = 192 * 1024,
            Tier = BclTier.Minimal,
            Console = "USB CDC",
        },
        new()
        {
            Id = "nucleo-f401re",
            Name = "Nucleo-F401RE",
            Chip = "STM32F401RE",
            Core = "Arm Cortex-M4F",
            Target = "thumbv7em-none-eabihf",
            FirmwareDirectory = "stm32f4",
            CargoFeature = "nucleo-f401re",
            Flash = FlashTool.ProbeRs,
            ProbeChip = "STM32F401RETx",
            RamBytes = 96 * 1024,
            HeapBytes = 64 * 1024,
            Tier = BclTier.None,
            Console = "USART2 on PA2, the ST-Link virtual COM port",
        },
    ];

    public static Board? ById(string id) =>
        All.FirstOrDefault(b => string.Equals(b.Id, id, StringComparison.OrdinalIgnoreCase));

    /// <summary>Boards that can actually execute a C# program.</summary>
    public static IEnumerable<Board> CanRunIl => All.Where(b => b.Tier != BclTier.None);
}
