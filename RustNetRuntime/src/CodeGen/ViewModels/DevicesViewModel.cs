using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using CodeGen.Models;
using CodeGen.Services;

namespace CodeGen.ViewModels;

/// <summary>
/// The Devices panel: what boards exist, which are attached, and putting
/// firmware or a program onto one.
///
/// Scanning is passive — it lists ports and probes and nothing more. Anything
/// that talks to a board (Identify) or writes to one (Flash, Deploy) is an
/// explicit action, because both reset the part and one of them erases flash.
/// </summary>
public sealed partial class DevicesViewModel : ObservableObject
{
    private readonly DeviceService _devices;
    private readonly BuildService _builds;
    private readonly ProjectService _projects;
    private readonly AppSettings _settings;
    private readonly Action<string> _log;

    public DevicesViewModel(
        DeviceService devices,
        BuildService builds,
        ProjectService projects,
        AppSettings settings,
        Action<string> log)
    {
        _devices = devices;
        _builds = builds;
        _projects = projects;
        _settings = settings;
        _log = log;

        foreach (var board in BoardCatalog.All)
        {
            Boards.Add(new DeviceViewModel(board));
        }
        Selected = Boards[0];
    }

    public ObservableCollection<DeviceViewModel> Boards { get; } = [];

    [ObservableProperty] private DeviceViewModel? _selected;
    [ObservableProperty] private bool _isBusy;
    [ObservableProperty] private string _summary = "Not scanned yet.";
    [ObservableProperty] private string _portsLine = "";

    /// <summary>
    /// The two thresholds, for the gauge legend.
    ///
    /// Instance rather than static because compiled bindings resolve against
    /// the data context, and a static property is not reachable from one.
    /// </summary>
    public string MinimalLabel => $"needs {BoardCatalog.TierBytes.Minimal:N0} B";
    public string FullLabel => $"needs {BoardCatalog.TierBytes.Full:N0} B";

    [RelayCommand]
    private async Task ScanAsync()
    {
        if (IsBusy) return;
        IsBusy = true;
        try
        {
            _devices.ForgetTools();
            var ports = await _devices.ListSerialPortsAsync().ConfigureAwait(true);
            PortsLine = ports.Count == 0
                ? "No serial ports."
                : string.Join("   ", ports.Select(p => p.PortName));

            var statuses = await _devices.RefreshAsync().ConfigureAwait(true);
            foreach (var status in statuses)
            {
                var row = Boards.FirstOrDefault(b => b.Board.Id == status.Board.Id);
                if (row is not null) row.Status = status;
            }

            var attached = statuses.Count(s => s.State is DeviceState.Connected or DeviceState.Ambiguous);
            Summary = attached == 0
                ? "Nothing attached."
                : $"{attached} board(s) may be attached.";
        }
        finally
        {
            IsBusy = false;
        }
    }

    [RelayCommand]
    private async Task IdentifyAsync()
    {
        if (Selected is null || IsBusy) return;
        IsBusy = true;
        Selected.IsBusy = true;
        try
        {
            _log($"— identifying {Selected.Name} —");
            var result = await _devices
                .IdentifyAsync(Selected.Board, Selected.Endpoint, _log)
                .ConfigureAwait(true);
            Selected.Identity = result.Succeeded ? result.Combined.Trim() : null;
            if (!result.Succeeded)
            {
                _log("Identify failed. The board may not be attached, or may need to be");
                _log("held in its bootloader while this runs.");
            }
        }
        finally
        {
            Selected.IsBusy = false;
            IsBusy = false;
        }
    }

    /// <summary>Builds and flashes the demonstration firmware.</summary>
    [RelayCommand]
    private Task FlashFirmwareAsync() => FlashAsync(applicationDll: null);

    /// <summary>
    /// Builds the open project and flashes it *into* the firmware image.
    ///
    /// These boards have no filesystem, so an application is not copied onto
    /// one — it is compiled into the image the firmware runs. `RUSTCLR_APP`
    /// tells the firmware's build script which assembly to embed.
    /// </summary>
    [RelayCommand]
    private async Task DeployApplicationAsync()
    {
        if (Selected is null || IsBusy) return;

        if (_projects.CurrentProjectPath is null)
        {
            _log("Open a project first — Deploy puts that project's assembly on the board.");
            return;
        }

        IsBusy = true;
        try
        {
            _log($"— building {_projects.CurrentProjectName} for {Selected.Name} —");
            var build = await _builds.BuildAsync(_settings, _log).ConfigureAwait(true);
            if (!build.Succeeded)
            {
                _log("Build failed; nothing was deployed.");
                return;
            }

            var assembly = _projects.FindOutputAssembly(_builds.Configuration);
            if (assembly is null)
            {
                _log("Build succeeded but no assembly was found.");
                return;
            }

            // Checking against the board's tier before flashing turns a puzzling
            // runtime message into an answer here, where the code is in front of
            // the user.
            if (Selected.Board.Tier == BclTier.Minimal)
            {
                _log($"{Selected.Name} carries the reduced binding set. Checking the program");
                _log("against those limits first — this is what --bcl minimal is for.");
                var probe = await _builds
                    .RunOnMinimalBclAsync(_settings, _log)
                    .ConfigureAwait(true);
                if (!probe.Succeeded)
                {
                    _log("");
                    _log("The program does not run against the reduced binding set, so it");
                    _log("will not run on this board either. Nothing was flashed.");
                    return;
                }
            }

            await FlashCoreAsync(assembly).ConfigureAwait(true);
        }
        finally
        {
            IsBusy = false;
        }
    }

    private async Task FlashAsync(string? applicationDll)
    {
        if (Selected is null || IsBusy) return;
        IsBusy = true;
        try
        {
            await FlashCoreAsync(applicationDll).ConfigureAwait(true);
        }
        finally
        {
            IsBusy = false;
        }
    }

    private async Task FlashCoreAsync(string? applicationDll)
    {
        if (Selected is null) return;
        Selected.IsBusy = true;
        try
        {
            _log($"— flashing {Selected.Name} —");
            var result = await _devices
                .FlashAsync(Selected.Board, applicationDll, Selected.Endpoint, _log)
                .ConfigureAwait(true);
            _log(result.Succeeded
                ? $"{Selected.Name} flashed."
                : $"{Selected.Name} was not flashed.");

            var statuses = await _devices.RefreshAsync().ConfigureAwait(true);
            var status = statuses.FirstOrDefault(s => s.Board.Id == Selected.Board.Id);
            if (status is not null) Selected.Status = status;
        }
        finally
        {
            Selected.IsBusy = false;
        }
    }

    [RelayCommand]
    private async Task MonitorAsync()
    {
        if (Selected is null || IsBusy) return;
        IsBusy = true;
        try
        {
            _log($"— monitoring {Selected.Name}; this runs until the board stops talking —");
            await _devices.MonitorAsync(Selected.Board, Selected.Endpoint, _log).ConfigureAwait(true);
        }
        finally
        {
            IsBusy = false;
        }
    }
}
