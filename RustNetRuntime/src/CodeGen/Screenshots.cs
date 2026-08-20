using Avalonia;
using Avalonia.Controls;
using Avalonia.Headless;
using Avalonia.Threading;
using CodeGen.Models;
using CodeGen.Services;
using CodeGen.ViewModels;
using CodeGen.Views;

namespace CodeGen;

/// <summary>
/// Renders the UI to PNG files without a display.
///
/// The README and the docs are supposed to show the app, and a screenshot that
/// nobody can regenerate goes stale the first time the layout changes. Running
/// `CodeGen --screenshot docs/images` re-renders every image from the real
/// windows, so the documentation cannot drift from the product.
///
/// Avalonia's headless platform draws through Skia here — the default headless
/// drawing backend produces blank frames, which would be worse than no image.
/// </summary>
internal static class Screenshots
{
    private sealed record Shot(string FileName, int Width, int Height, Func<Window> Build);

    public static int Capture(string directory)
    {
        Directory.CreateDirectory(directory);

        try
        {
            AppBuilder.Configure<App>()
                .UseHeadless(new AvaloniaHeadlessPlatformOptions { UseHeadlessDrawing = false })
                .UseSkia()
                .WithInterFont()
                .SetupWithoutStarting();
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"Could not start the headless renderer: {ex.Message}");
            return 1;
        }

        var sample = BuildSampleWorkspace();

        var shots = new List<Shot>
        {
            new("codegen-main.png", 1440, 900, () => MainWindowWith(sample, showChat: true)),
            new("codegen-chat.png", 1440, 900, () => MainWindowWith(sample, showChat: true, chatFocus: true)),
            new("codegen-new-project.png", 760, 560, () => new NewProjectWindow()),
            new("codegen-settings.png", 700, 640, () => new SettingsWindow(SampleSettings())),
        };

        var written = 0;
        foreach (var shot in shots)
        {
            try
            {
                var path = Path.Combine(directory, shot.FileName);
                Render(shot, path);
                Console.WriteLine($"wrote {path}");
                written++;
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine($"{shot.FileName}: {ex.Message}");
            }
        }

        Console.WriteLine($"{written} of {shots.Count} screenshots rendered.");
        return written == shots.Count ? 0 : 1;
    }

    private static void Render(Shot shot, string path)
    {
        var window = shot.Build();
        window.Width = shot.Width;
        window.Height = shot.Height;
        window.Show();

        // Let bindings settle and the first layout pass complete before the
        // frame is grabbed; otherwise the capture catches an empty window.
        for (var i = 0; i < 8; i++)
        {
            Dispatcher.UIThread.RunJobs();
        }

        var frame = window.CaptureRenderedFrame();
        if (frame is null)
        {
            throw new InvalidOperationException("the renderer returned no frame");
        }

        using var stream = File.Create(path);
        frame.Save(stream);
        window.Close();
    }

    private static Window MainWindowWith(string projectPath, bool showChat, bool chatFocus = false)
    {
        var viewModel = new MainWindowViewModel();
        viewModel.Settings.LastProject = projectPath;
        viewModel.ChatPanelVisible = showChat;

        // A screenshot of an empty IDE teaches nothing, so the window is shown
        // over a real generated project with a real transcript.
        viewModel.OpenProjectAtForScreenshot(projectPath);

        // Show the editor doing its job rather than its empty state.
        foreach (var file in new[] { "Gateway.cs", "Program.cs" })
        {
            var path = Path.Combine(projectPath, file);
            if (File.Exists(path)) viewModel.OpenFile(path);
        }
        viewModel.SelectedTab = viewModel.Tabs.FirstOrDefault();

        viewModel.Log("$ dotnet build -c Release \"SensorGateway.csproj\"");
        viewModel.Log("  SensorGateway -> bin/Release/net10.0/SensorGateway.dll");
        viewModel.Log("Build succeeded.");
        viewModel.Log("$ rustnet run \"SensorGateway.dll\" --stats");
        viewModel.Log("publish temp-1 mean=18.4375");
        viewModel.Log("publish temp-1 mean=19.9375");
        viewModel.Status = "Ran on RustCLR";
        viewModel.Telemetry = "IL 4,812   HEAP 3.1 KB   GC 0";
        viewModel.CursorPosition = "Ln 24, Col 9";

        if (chatFocus)
        {
            // Prefer a transcript captured from a real assistant turn, so the
            // screenshot shows what the product actually did rather than an
            // invented conversation. `CodeGen --chat "…"` writes one.
            var captured = Headless.LoadTranscript();
            if (captured is { Entries.Count: > 0 } real)
            {
                foreach (var entry in real.Entries)
                {
                    viewModel.ChatEntries.Add(entry);
                }
                viewModel.SelectedModel = real.Model;
            }
            else
            {
                viewModel.ChatEntries.Add(new ChatEntry
                {
                    Role = "note",
                    Text =
                        "No captured transcript. Run CodeGen --chat first, so this screenshot "
                        + "shows a real conversation rather than an invented one.",
                });
            }
        }

        return new MainWindow { DataContext = viewModel };
    }

    private static AppSettings SampleSettings()
    {
        var settings = new AppSettings
        {
            ActiveProvider = LlmProvider.Claude,
            Temperature = 0.2,
            MaxTokens = 8192,
            SystemPrompt =
                "You are Jack — The Code Bender, the coding assistant inside CodeGen, the IDE "
                + "for RustNetRuntime (C# running on RustCLR, a CLR rebuilt in Rust).",
            TavilyApiKey = "tvly-••••••••••••••••",
            RustNetPath = @"target\release\rustnet.exe",
        };

        foreach (var provider in Enum.GetValues<LlmProvider>())
        {
            settings.Providers[provider] = new ProviderSettings
            {
                Provider = provider,
                Model = AppSettings.KnownModels(provider).FirstOrDefault() ?? "",
                ApiKey = provider == LlmProvider.Ollama ? "ollama" : "••••••••••••",
                Endpoint = provider switch
                {
                    LlmProvider.OpenAI => "https://api.openai.com/v1",
                    LlmProvider.Claude => "https://api.anthropic.com",
                    LlmProvider.Gemini => "https://generativelanguage.googleapis.com/v1beta/openai",
                    _ => "http://localhost:11434/v1",
                },
            };
        }
        return settings;
    }

    /// <summary>
    /// Generates a throwaway project so the explorer and editor have real
    /// content. Written under the temp directory, never into the repository.
    /// </summary>
    public static string BuildSampleWorkspace()
    {
        var root = Path.Combine(Path.GetTempPath(), "codegen-screenshot-workspace");
        var project = Path.Combine(root, "SensorGateway");

        // Reuse an existing workspace. That lets `--chat --project <this>` make
        // real edits between two screenshot runs, so the transcript and the
        // files on screen describe the same work.
        if (Directory.Exists(project) && Directory.EnumerateFiles(project).Any())
        {
            return project;
        }

        Directory.CreateDirectory(root);
        var projects = new ProjectService();
        var template = TemplateCatalog.Find("iot-gateway") ?? TemplateCatalog.Blank;
        projects.Create(root, "SensorGateway", template);
        return projects.CurrentProjectPath ?? project;
    }
}
