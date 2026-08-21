using System.Collections.ObjectModel;
using System.Globalization;
using System.Text.RegularExpressions;
using CodeGen.Models;
using CodeGen.Services;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace CodeGen.ViewModels;

/// <summary>
/// The dialogs the view owns. The view model decides *when* to ask; the view
/// decides *how*, because file pickers and windows are platform concerns.
/// </summary>
public interface IWorkspaceDialogs
{
    Task<string?> PickFolderAsync(string title);
    Task<string?> PickFileAsync(string title);
    Task<IReadOnlyList<string>> PickImagesAsync();
    Task<NewProjectRequest?> NewProjectAsync();
    Task ShowSettingsAsync(AppSettings settings);
    Task ShowDevicesAsync(DevicesViewModel devices);
    Task<int?> AskLineNumberAsync(int maxLine);
    Task ShowMessageAsync(string title, string message);
}

/// <summary>What the New Project dialog collected.</summary>
public sealed record NewProjectRequest(string ParentDirectory, string Name, ProjectTemplate Template);

public sealed partial class MainWindowViewModel : ObservableObject
{
    private readonly ConfigurationService _configuration = new();
    private readonly ProjectService _projects = new();
    private readonly ProcessRunner _runner = new();
    private readonly BuildService _builds;
    private readonly KernelService _kernel;

    private CancellationTokenSource? _running;

    public MainWindowViewModel()
    {
        Settings = _configuration.Load();
        _builds = new BuildService(_projects, _runner);
        _kernel = new KernelService(_projects, _builds, _runner, Log);

        ChatEntries = [];
        Attachments = [];

        ChatPanelVisible = Settings.ChatPanelVisible;
        ExplorerVisible = Settings.ExplorerVisible;
        LogPanelVisible = Settings.LogPanelVisible;
        ShowLineNumbers = Settings.ShowLineNumbers;
        ChatPanelWidth = Settings.ChatPanelWidth;
        ExplorerWidth = Settings.ExplorerWidth;
        LogPanelHeight = Settings.LogPanelHeight;
        SelectedProvider = Settings.ActiveProvider;
        SelectedModel = Settings.Active.Model;

        Log($"CodeGen · RustCLR — built by Gravicode Studios, led by Kang Fadhil");
        Log($"Settings: {_configuration.ConfigurationPath}");

        if (!string.IsNullOrWhiteSpace(Settings.LastProject) && Directory.Exists(Settings.LastProject))
        {
            OpenProjectAt(Settings.LastProject);
        }
        else
        {
            Status = "No project open. File → New Project to begin.";
        }
    }

    public AppSettings Settings { get; }

    /// <summary>Set by the view once it can show dialogs.</summary>
    public IWorkspaceDialogs? Dialogs { get; set; }

    // ── Panels ────────────────────────────────────────────────────────────
    [ObservableProperty] private bool _chatPanelVisible = true;
    [ObservableProperty] private bool _explorerVisible = true;
    [ObservableProperty] private bool _logPanelVisible = true;
    [ObservableProperty] private double _chatPanelWidth = 380;
    [ObservableProperty] private double _explorerWidth = 240;
    [ObservableProperty] private double _logPanelHeight = 160;
    [ObservableProperty] private bool _showLineNumbers = true;

    // ── Status bar ────────────────────────────────────────────────────────
    [ObservableProperty] private string _status = "Ready";
    [ObservableProperty] private string _cursorPosition = "Ln 1, Col 1";
    [ObservableProperty] private string _projectLabel = "no project";
    [ObservableProperty] private bool _isBusy;

    /// <summary>
    /// The signature readout: what the runtime actually did on the last run.
    /// An IDE for a runtime project should show the runtime's own numbers.
    /// </summary>
    [ObservableProperty] private string _telemetry = "IL —   HEAP —   GC —";

    // ── Explorer ──────────────────────────────────────────────────────────
    public ObservableCollection<FileNodeViewModel> ExplorerRoots { get; } = [];

    // ── Editor ────────────────────────────────────────────────────────────
    public ObservableCollection<EditorTabViewModel> Tabs { get; } = [];

    [ObservableProperty] private EditorTabViewModel? _selectedTab;

    // ── Chat ──────────────────────────────────────────────────────────────
    public ObservableCollection<ChatEntry> ChatEntries { get; }
    public ObservableCollection<string> Attachments { get; }

    [ObservableProperty] private string _chatInput = "";
    [ObservableProperty] private bool _chatBusy;
    [ObservableProperty] private LlmProvider _selectedProvider;
    [ObservableProperty] private string _selectedModel = "";

    public IReadOnlyList<LlmProvider> Providers { get; } = Enum.GetValues<LlmProvider>();

    public ObservableCollection<string> ModelChoices { get; } = [];

    // ── Logs ──────────────────────────────────────────────────────────────
    public ObservableCollection<string> LogLines { get; } = [];

    // ══ Project ═══════════════════════════════════════════════════════════

    [RelayCommand]
    private async Task NewProjectAsync()
    {
        if (Dialogs is null) return;
        var request = await Dialogs.NewProjectAsync();
        if (request is null) return;

        try
        {
            var opened = _projects.Create(request.ParentDirectory, request.Name, request.Template);
            RefreshExplorer();
            OpenFile(opened);
            Settings.LastProject = _projects.CurrentProjectPath ?? "";
            Status = $"Created {request.Name} from {request.Template.Name}";
            Log($"Created {request.Name} from the {request.Template.Name} template");
            Log($"Run it with: {request.Template.RunHint.Replace("{NAME}", request.Name)}");
            _kernel.ResetThread(Settings);
            SyncChat();
        }
        catch (Exception ex)
        {
            await Dialogs.ShowMessageAsync("Could not create the project", ex.Message);
        }
    }

    [RelayCommand]
    private async Task OpenProjectAsync()
    {
        if (Dialogs is null) return;
        var folder = await Dialogs.PickFolderAsync("Open project folder");
        if (folder is null) return;
        OpenProjectAt(folder);
    }

    /// <summary>
    /// Opens a project without going through a dialog. Used by the screenshot
    /// harness, which has no user to ask.
    /// </summary>
    public void OpenProjectAtForScreenshot(string folder) => OpenProjectAt(folder);

    private void OpenProjectAt(string folder)
    {
        try
        {
            _projects.Open(folder);
            RefreshExplorer();
            Settings.LastProject = folder;
            ProjectLabel = _projects.CurrentProjectName ?? "no project";
            Status = $"Opened {ProjectLabel}";
            Log($"Opened project {folder}");
            _kernel.ResetThread(Settings);
            SyncChat();
        }
        catch (Exception ex)
        {
            Status = ex.Message;
            Log($"Could not open {folder}: {ex.Message}");
        }
    }

    [RelayCommand]
    private async Task OpenFileAsync()
    {
        if (Dialogs is null) return;
        var file = await Dialogs.PickFileAsync("Open file");
        if (file is not null) OpenFile(file);
    }

    [RelayCommand]
    private void CloseProject()
    {
        _projects.Close();
        ExplorerRoots.Clear();
        Tabs.Clear();
        SelectedTab = null;
        ProjectLabel = "no project";
        Settings.LastProject = "";
        Status = "Project closed";
    }

    public void OpenFile(string path)
    {
        var existing = Tabs.FirstOrDefault(t =>
            string.Equals(t.FilePath, path, StringComparison.OrdinalIgnoreCase));
        if (existing is not null)
        {
            SelectedTab = existing;
            return;
        }

        try
        {
            var tab = new EditorTabViewModel(path);
            Tabs.Add(tab);
            SelectedTab = tab;
            Status = Path.GetFileName(path);
        }
        catch (Exception ex)
        {
            Log($"Could not open {path}: {ex.Message}");
        }
    }

    [RelayCommand]
    private void CloseTab(EditorTabViewModel? tab)
    {
        tab ??= SelectedTab;
        if (tab is null) return;
        var index = Tabs.IndexOf(tab);
        Tabs.Remove(tab);
        SelectedTab = Tabs.Count == 0 ? null : Tabs[Math.Clamp(index, 0, Tabs.Count - 1)];
    }

    [RelayCommand]
    private void Save()
    {
        if (SelectedTab is null) return;
        SelectedTab.Save();
        Status = $"Saved {SelectedTab.FileName}";
    }

    [RelayCommand]
    private void SaveAll()
    {
        var saved = 0;
        foreach (var tab in Tabs.Where(t => t.IsDirty))
        {
            tab.Save();
            saved++;
        }
        Status = saved == 0 ? "Nothing to save" : $"Saved {saved} file(s)";
    }

    private void RefreshExplorer()
    {
        ExplorerRoots.Clear();
        var tree = _projects.BuildTree();
        if (tree is null) return;

        var root = new FileNodeViewModel(tree) { IsExpanded = true };
        // Expand one level in: a tree showing only the project name tells the
        // reader nothing they did not already know.
        foreach (var child in root.Children.Where(c => c.IsDirectory))
        {
            child.IsExpanded = true;
        }
        ExplorerRoots.Add(root);
        ProjectLabel = _projects.CurrentProjectName ?? "no project";
    }

    [RelayCommand]
    private void RefreshTree()
    {
        RefreshExplorer();
        foreach (var tab in Tabs) tab.Reload();
        Status = "Refreshed";
    }

    // ══ Editing ═══════════════════════════════════════════════════════════

    [RelayCommand]
    private async Task GoToLineAsync()
    {
        if (Dialogs is null || SelectedTab is null) return;
        var line = await Dialogs.AskLineNumberAsync(SelectedTab.Document.LineCount);
        if (line is null) return;
        RequestGoToLine?.Invoke(line.Value);
    }

    /// <summary>Raised when the editor should move the caret. The view listens.</summary>
    public event Action<int>? RequestGoToLine;

    /// <summary>
    /// Reindents the open file.
    ///
    /// This is a brace-depth reformatter, not a C# parser: it fixes indentation
    /// and trailing whitespace and leaves everything else alone. Anything more
    /// would need Roslyn, and silently rewriting code is worse than not
    /// formatting it.
    /// </summary>
    [RelayCommand]
    private void FormatCode()
    {
        if (SelectedTab is null) return;

        var indentUnit = new string(' ', Math.Clamp(Settings.TabSize, 1, 8));
        var lines = SelectedTab.Document.Text.Replace("\r\n", "\n").Split('\n');
        var formatted = new List<string>(lines.Length);
        var depth = 0;
        var inBlockComment = false;

        foreach (var raw in lines)
        {
            var line = raw.Trim();

            if (line.Length == 0)
            {
                formatted.Add("");
                continue;
            }

            // Leave the interior of block comments and verbatim strings alone.
            if (inBlockComment)
            {
                formatted.Add(raw.TrimEnd());
                if (line.Contains("*/")) inBlockComment = false;
                continue;
            }
            if (line.StartsWith("/*"))
            {
                formatted.Add(new string(' ', depth * indentUnit.Length) + line);
                if (!line.Contains("*/")) inBlockComment = true;
                continue;
            }

            var opens = CountOutsideStrings(line, '{');
            var closes = CountOutsideStrings(line, '}');

            // A line that starts by closing dedents before it is written.
            if (line.StartsWith('}') || line.StartsWith(')') || line.StartsWith(']'))
            {
                depth = Math.Max(0, depth - 1);
            }

            formatted.Add(string.Concat(Enumerable.Repeat(indentUnit, depth)) + line);

            var delta = opens - closes;
            if (line.StartsWith('}') && delta > 0) delta -= 0; // already dedented above
            depth = Math.Max(0, depth + (line.StartsWith('}') ? Math.Max(delta, 0) : delta));
        }

        var caret = SelectedTab.Document.Text.Length;
        SelectedTab.Document.Text = string.Join(Environment.NewLine, formatted);
        Status = "Formatted";
        _ = caret;
    }

    /// <summary>Counts a brace, skipping ones inside string or char literals.</summary>
    private static int CountOutsideStrings(string line, char target)
    {
        var count = 0;
        var inString = false;
        var inChar = false;
        for (var i = 0; i < line.Length; i++)
        {
            var c = line[i];
            if (c == '\\' && (inString || inChar)) { i++; continue; }
            if (c == '"' && !inChar) inString = !inString;
            else if (c == '\'' && !inString) inChar = !inChar;
            else if (c == target && !inString && !inChar) count++;
            else if (c == '/' && i + 1 < line.Length && line[i + 1] == '/' && !inString && !inChar) break;
        }
        return count;
    }

    // ══ Build and run ═════════════════════════════════════════════════════

    [RelayCommand]
    private async Task BuildAsync()
    {
        SaveAll();
        await WithBusy("Building", async token =>
        {
            var result = await _builds.BuildAsync(Settings, Log, token);
            Status = result.Succeeded
                ? $"Build succeeded in {result.Duration.TotalSeconds:0.0}s"
                : "Build failed";
        });
    }

    [RelayCommand]
    private Task RunAsync() => RunOn(RunTarget.Dotnet);

    [RelayCommand]
    private Task RunOnRustClrAsync() => RunOn(RunTarget.RustClr);

    private Task RunOn(RunTarget target)
    {
        SaveAll();
        var label = target == RunTarget.RustClr ? "RustCLR" : ".NET";
        return WithBusy($"Running on {label}", async token =>
        {
            var result = await _builds.RunAsync(Settings, target, Log, token);
            Status = result.Succeeded ? $"Ran on {label}" : $"Run on {label} failed ({result.ExitCode})";
            if (target == RunTarget.RustClr) UpdateTelemetry(result.Combined);
        });
    }

    [RelayCommand]
    private async Task VerifyAsync()
    {
        await WithBusy("Verifying on RustCLR", async token =>
        {
            var result = await _builds.VerifyOnRustClrAsync(Settings, Log, token);
            Status = result.Succeeded ? "Everything resolves on RustCLR" : "RustCLR reported gaps";
        });
    }

    [RelayCommand]
    private async Task DeployAsync()
    {
        var rid = OperatingSystem.IsWindows() ? "win-x64"
            : OperatingSystem.IsMacOS() ? "osx-arm64"
            : "linux-x64";

        await WithBusy($"Publishing for {rid}", async token =>
        {
            var result = await _builds.DeployAsync(Settings, rid, Log, token);
            Status = result.Succeeded ? $"Published for {rid}" : "Publish failed";
        });
    }

    [RelayCommand]
    private void Cancel()
    {
        _running?.Cancel();
        Status = "Cancelling…";
    }

    private async Task WithBusy(string what, Func<CancellationToken, Task> work)
    {
        if (_projects.CurrentProjectPath is null)
        {
            Status = "Open a project first";
            return;
        }
        if (IsBusy) return;

        IsBusy = true;
        Status = what + "…";
        _running = new CancellationTokenSource();
        try
        {
            await work(_running.Token);
        }
        catch (Exception ex)
        {
            Log($"{what} failed: {ex.Message}");
            Status = $"{what} failed";
        }
        finally
        {
            IsBusy = false;
            _running?.Dispose();
            _running = null;
            RefreshExplorer();
        }
    }

    /// <summary>
    /// Reads the runtime counters out of `rustnet run --stats` output.
    /// </summary>
    private void UpdateTelemetry(string output)
    {
        var il = MatchNumber(output, @"IL instructions\s+([\d,]+)");
        var heap = MatchNumber(output, @"live bytes\s+([\d,]+)");
        var collections = MatchNumber(output, @"collections\s+([\d,]+)");

        if (il is null && heap is null && collections is null) return;

        Telemetry = $"IL {il ?? "—"}   HEAP {FormatBytes(heap)}   GC {collections ?? "—"}";
    }

    private static string? MatchNumber(string text, string pattern)
    {
        var match = Regex.Match(text, pattern, RegexOptions.IgnoreCase);
        return match.Success ? match.Groups[1].Value.Trim() : null;
    }

    private static string FormatBytes(string? raw)
    {
        if (raw is null) return "—";
        if (!long.TryParse(raw.Replace(",", ""), NumberStyles.Integer, CultureInfo.InvariantCulture, out var bytes))
        {
            return raw;
        }
        return bytes < 1024 ? $"{bytes} B"
            : bytes < 1024 * 1024 ? $"{bytes / 1024.0:0.0} KB"
            : $"{bytes / (1024.0 * 1024):0.0} MB";
    }

    // ══ Chat ══════════════════════════════════════════════════════════════

    [RelayCommand]
    private async Task SendChatAsync()
    {
        var message = ChatInput.Trim();
        if (message.Length == 0 || ChatBusy) return;

        ChatInput = "";
        ChatBusy = true;
        Status = "Jack is working…";

        try
        {
            var attachments = Attachments.ToList();
            Attachments.Clear();
            await _kernel.SendAsync(Settings, message, attachments);
            SyncChat();
            RefreshExplorer();
            foreach (var tab in Tabs) tab.Reload();
            Status = "Ready";
        }
        finally
        {
            ChatBusy = false;
        }
    }

    [RelayCommand]
    private void ClearChat()
    {
        _kernel.ResetThread(Settings);
        Attachments.Clear();
        SyncChat();
        Status = "Thread cleared";
    }

    [RelayCommand]
    private async Task AttachImageAsync()
    {
        if (Dialogs is null) return;
        foreach (var path in await Dialogs.PickImagesAsync())
        {
            if (!Attachments.Contains(path)) Attachments.Add(path);
        }
    }

    [RelayCommand]
    private void RemoveAttachment(string? path)
    {
        if (path is not null) Attachments.Remove(path);
    }

    private void SyncChat()
    {
        ChatEntries.Clear();
        foreach (var entry in _kernel.Transcript) ChatEntries.Add(entry);
    }

    partial void OnSelectedProviderChanged(LlmProvider value)
    {
        Settings.ActiveProvider = value;
        ModelChoices.Clear();
        foreach (var model in AppSettings.KnownModels(value)) ModelChoices.Add(model);

        var configured = Settings.Providers[value].Model;
        if (!string.IsNullOrWhiteSpace(configured) && !ModelChoices.Contains(configured))
        {
            ModelChoices.Insert(0, configured);
        }
        SelectedModel = configured;
    }

    partial void OnSelectedModelChanged(string value)
    {
        if (string.IsNullOrWhiteSpace(value)) return;

        // A model configured by hand — a self-hosted endpoint, a provider whose
        // catalogue moved on — is still a valid choice, so make sure the picker
        // can show it rather than falling blank.
        if (!ModelChoices.Contains(value)) ModelChoices.Insert(0, value);

        Settings.Providers[Settings.ActiveProvider].Model = value;
    }

    // ══ Settings and layout ═══════════════════════════════════════════════

    [RelayCommand]
    private async Task OpenSettingsAsync()
    {
        if (Dialogs is null) return;
        await Dialogs.ShowSettingsAsync(Settings);
        PersistSettings();

        SelectedProvider = Settings.ActiveProvider;
        SelectedModel = Settings.Active.Model;
        ShowLineNumbers = Settings.ShowLineNumbers;
        Status = "Settings saved";
    }

    /// <summary>
    /// Opens the Devices panel.
    ///
    /// It shares this view model's log sink, so a flash reports into the same
    /// place a build does — one running account of what the IDE did.
    /// </summary>
    [RelayCommand]
    private async Task OpenDevicesAsync()
    {
        if (Dialogs is null) return;

        var devices = new DeviceService(_runner) { RepositoryRoot = FindRepositoryRoot() };
        var model = new DevicesViewModel(devices, _builds, _projects, Settings, Log);
        LogPanelVisible = true;
        await Dialogs.ShowDevicesAsync(model);
    }

    /// <summary>
    /// Walks up from the open project, then from the executable, looking for
    /// the runtime repository.
    ///
    /// `embedded/` and `crates/` together are the signature — either alone
    /// appears in plenty of unrelated trees.
    /// </summary>
    private string? FindRepositoryRoot()
    {
        foreach (var start in (string?[])[_projects.CurrentProjectPath, AppContext.BaseDirectory])
        {
            if (start is null) continue;
            for (var directory = new DirectoryInfo(start); directory is not null; directory = directory.Parent)
            {
                if (Directory.Exists(Path.Combine(directory.FullName, "embedded"))
                    && Directory.Exists(Path.Combine(directory.FullName, "crates")))
                {
                    return directory.FullName;
                }
            }
        }
        return null;
    }

    [RelayCommand] private void ToggleChat() => ChatPanelVisible = !ChatPanelVisible;
    [RelayCommand] private void ToggleExplorer() => ExplorerVisible = !ExplorerVisible;
    [RelayCommand] private void ToggleLogs() => LogPanelVisible = !LogPanelVisible;

    [RelayCommand]
    private void ToggleLineNumbers()
    {
        ShowLineNumbers = !ShowLineNumbers;
        Settings.ShowLineNumbers = ShowLineNumbers;
    }

    [RelayCommand]
    private void ClearLog() => LogLines.Clear();

    /// <summary>Writes layout and workspace state back to app.config.</summary>
    public void PersistSettings()
    {
        Settings.ChatPanelVisible = ChatPanelVisible;
        Settings.ExplorerVisible = ExplorerVisible;
        Settings.LogPanelVisible = LogPanelVisible;
        Settings.ChatPanelWidth = ChatPanelWidth;
        Settings.ExplorerWidth = ExplorerWidth;
        Settings.LogPanelHeight = LogPanelHeight;
        Settings.ShowLineNumbers = ShowLineNumbers;
        Settings.LastProject = _projects.CurrentProjectPath ?? "";

        _configuration.Save(Settings);
        if (_configuration.LastWriteError is { } problem)
        {
            Log($"Settings could not be written: {problem}");
        }
    }

    /// <summary>Appends a line to the log panel, from any thread.</summary>
    public void Log(string line)
    {
        void Append()
        {
            LogLines.Add(line);
            // Keep the panel bounded; a long build can emit thousands of lines.
            while (LogLines.Count > 2000) LogLines.RemoveAt(0);
        }

        if (Avalonia.Threading.Dispatcher.UIThread.CheckAccess()) Append();
        else Avalonia.Threading.Dispatcher.UIThread.Post(Append);
    }
}
