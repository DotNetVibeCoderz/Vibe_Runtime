using Avalonia;
using Avalonia.Media;
using CommunityToolkit.Mvvm.ComponentModel;
using CodeGen.Models;

namespace CodeGen.ViewModels;

/// <summary>
/// One board in the Devices panel.
///
/// The panel's organising idea is that a board's identity here *is* its memory
/// budget: whether it can run a C# program at all, and how much of RustBCL it
/// can hold, is decided by two measured numbers. So each row draws its heap
/// against those two thresholds rather than printing a tier name and leaving
/// the reader to take it on faith.
/// </summary>
public sealed partial class DeviceViewModel : ObservableObject
{
    /// <summary>
    /// Pixel width of the budget track.
    ///
    /// A fixed width rather than a proportional one, so every board's bar is
    /// drawn to the same scale and two rows can be compared by eye. That is the
    /// whole point of the gauge.
    /// </summary>
    public const double TrackWidth = 260;

    /// <summary>
    /// Bytes the track spans. 288 KB is the largest heap any board here is
    /// given that still fits on screen usefully; the K210's megabyte is clamped
    /// and labelled rather than allowed to flatten everything else.
    /// </summary>
    private const double TrackBytes = 288 * 1024;

    public DeviceViewModel(Board board)
    {
        Board = board;
        Status = new DeviceStatus
        {
            Board = board,
            State = DeviceState.NotConnected,
            Detail = "not scanned yet",
        };
    }

    public Board Board { get; }

    [ObservableProperty] private DeviceStatus _status;
    [ObservableProperty] private bool _isBusy;

    /// <summary>Set when the board reported its own identity.</summary>
    [ObservableProperty] private string? _identity;

    public string Name => Board.Name;
    public string Chip => Board.Chip;
    public string Core => Board.Core;
    public string Console => Board.Console;
    public string Target => Board.Target;

    public string HeapLabel => $"{Board.HeapBytes / 1024} KB heap";
    public string RamLabel => $"{Board.RamBytes / 1024} KB RAM";

    /// <summary>How far along the track this board's heap reaches.</summary>
    public double FillWidth => Math.Clamp(Board.HeapBytes / TrackBytes, 0, 1) * TrackWidth;

    /// <summary>Where 192,045 bytes falls — the reduced binding set.</summary>
    public static double MinimalMarkerX => BoardCatalog.TierBytes.Minimal / TrackBytes * TrackWidth;

    /// <summary>Where 260,702 bytes falls — every binding.</summary>
    public static double FullMarkerX => BoardCatalog.TierBytes.Full / TrackBytes * TrackWidth;

    /// <summary>Left offsets for the two gradations, as the layout wants them.</summary>
    public Thickness MinimalMarkerMargin { get; } = new(MinimalMarkerX, 0, 0, 0);
    public Thickness FullMarkerMargin { get; } = new(FullMarkerX, 0, 0, 0);

    /// <summary>
    /// The tier, said as arithmetic rather than as a verdict.
    /// </summary>
    public string BudgetLine
    {
        get
        {
            var (minimal, full) = BoardCatalog.TierBytes;
            return Board.Tier switch
            {
                BclTier.Full => $"{Board.HeapBytes:N0} bytes — clears {full:N0}, so every binding fits",
                BclTier.Minimal =>
                    $"{Board.HeapBytes:N0} bytes — clears {minimal:N0} by {Board.HeapBytes - minimal:N0}, "
                    + "so console, strings and maths fit",
                _ => $"{Board.HeapBytes:N0} bytes — short of {minimal:N0} by {minimal - Board.HeapBytes:N0}, "
                    + "so no program runs",
            };
        }
    }

    /// <summary>
    /// Palette key for the tier.
    ///
    /// Reuses the existing accents semantically rather than adding colours:
    /// patina reads as settled and correct, amber as a working limit, ember as
    /// a stop. Nothing here is decorative — the colour *is* the answer to "will
    /// my program run on this".
    /// </summary>
    public IBrush TierBrush => Themed(Board.Tier switch
    {
        BclTier.Full => "Patina",
        BclTier.Minimal => "Amber",
        _ => "Ember",
    });

    public IBrush StateBrush => Themed(Status.State switch
    {
        DeviceState.Connected => "Patina",
        DeviceState.Ambiguous => "Amber",
        DeviceState.ToolMissing => "Oxide",
        _ => "Muted",
    });

    /// <summary>
    /// Resolves a brush from the Forge theme by name.
    ///
    /// Looked up rather than hard-coded so the panel cannot drift from the rest
    /// of the app's palette. Falls back to grey when there is no application —
    /// which happens under the headless screenshot renderer, and is not worth
    /// throwing over.
    /// </summary>
    private static IBrush Themed(string key)
    {
        if (Application.Current is { } app
            && app.Resources.TryGetResource(key, app.ActualThemeVariant, out var found)
            && found is IBrush brush)
        {
            return brush;
        }
        return Brushes.Gray;
    }

    public string TierLabel => Board.TierLabel;
    public string StateLabel => Status.StateLabel;
    public string Detail => Status.Detail;
    public string? Endpoint => Status.Endpoint;

    public string FirmwareLine => Status.Firmware is { } firmware
        ? $"{firmware.SizeLabel}, built {firmware.BuiltLabel}"
        : "not built";

    public bool CanRunPrograms => Board.Tier != BclTier.None;

    /// <summary>Raised when the underlying status is replaced.</summary>
    partial void OnStatusChanged(DeviceStatus value)
    {
        _ = value;
        OnPropertyChanged(nameof(StateLabel));
        OnPropertyChanged(nameof(StateBrush));
        OnPropertyChanged(nameof(Detail));
        OnPropertyChanged(nameof(Endpoint));
        OnPropertyChanged(nameof(FirmwareLine));
    }
}
