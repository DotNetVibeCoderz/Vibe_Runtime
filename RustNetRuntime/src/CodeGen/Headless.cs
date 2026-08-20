using System.Text.Json;
using CodeGen.Models;
using CodeGen.Services;

namespace CodeGen;

/// <summary>
/// Command-line entry points for driving CodeGen without a window.
///
/// Two jobs: configuring the app from a script, and running a real assistant
/// turn so the integration can be exercised in CI or from a terminal. Both go
/// through exactly the same services the UI uses — there is no parallel code
/// path that could pass here and fail in the app.
/// </summary>
internal static class Headless
{
    /// <summary>Where a headless chat writes its transcript for later rendering.</summary>
    public static string TranscriptPath =>
        Path.Combine(AppContext.BaseDirectory, "last-transcript.json");

    /// <summary>
    /// `--set Key=Value [Key=Value…]` — writes settings into app.config.
    ///
    /// This is the same store the Settings dialog writes, which is why secrets
    /// can be configured on a build machine without a GUI, and why they land in
    /// the generated <c>CodeGen.dll.config</c> rather than in the repository.
    /// </summary>
    public static int Set(string[] pairs)
    {
        var configuration = new ConfigurationService();
        var settings = configuration.Load();
        var applied = 0;

        foreach (var pair in pairs)
        {
            var separator = pair.IndexOf('=');
            if (separator <= 0)
            {
                Console.Error.WriteLine($"Skipping '{pair}' — expected Key=Value.");
                continue;
            }

            var key = pair[..separator].Trim();
            var value = pair[(separator + 1)..];

            if (!Apply(settings, key, value))
            {
                Console.Error.WriteLine($"Unknown setting '{key}'.");
                continue;
            }
            applied++;
            Console.WriteLine($"  {key} = {Redact(key, value)}");
        }

        configuration.Save(settings);
        if (configuration.LastWriteError is { } problem)
        {
            Console.Error.WriteLine($"Could not write settings: {problem}");
            return 1;
        }

        Console.WriteLine($"{applied} setting(s) written to {configuration.ConfigurationPath}");
        return applied > 0 ? 0 : 1;
    }

    /// <summary>
    /// `--chat "<prompt>" [--project <dir>]` — runs one real assistant turn.
    ///
    /// The provider, model and key come from app.config, so this exercises the
    /// live LLM, the kernel functions and the file system exactly as the chat
    /// panel does.
    /// </summary>
    public static int Chat(string prompt, string? projectDirectory)
    {
        var configuration = new ConfigurationService();
        var settings = configuration.Load();

        var projects = new ProjectService();
        var runner = new ProcessRunner();
        var builds = new BuildService(projects, runner);

        void Log(string line) => Console.WriteLine($"  · {line}");
        var kernel = new KernelService(projects, builds, runner, Log);

        if (!string.IsNullOrWhiteSpace(projectDirectory))
        {
            Directory.CreateDirectory(projectDirectory);
            projects.Open(projectDirectory);
            Console.WriteLine($"project  {projects.CurrentProjectPath}");
        }

        kernel.EnsureConfigured(settings);
        if (!kernel.IsReady)
        {
            Console.Error.WriteLine($"Assistant not configured: {kernel.ConfigurationProblem}");
            return 1;
        }

        Console.WriteLine($"provider {settings.Active.DisplayName} · {settings.Active.Model}");
        Console.WriteLine($"tools    {kernel.AvailableTools().Count} registered");
        Console.WriteLine();
        Console.WriteLine($"> {prompt}");
        Console.WriteLine();

        var started = DateTimeOffset.Now;
        var reply = kernel.SendAsync(settings, prompt).GetAwaiter().GetResult();
        var elapsed = DateTimeOffset.Now - started;

        Console.WriteLine(reply.Text);
        Console.WriteLine();
        if (reply.HasTools)
        {
            Console.WriteLine($"tools used: {reply.ToolSummary}");
        }
        Console.WriteLine($"elapsed: {elapsed.TotalSeconds:0.0}s");

        SaveTranscript(kernel.Transcript, settings);
        return reply.IsSystemNote ? 1 : 0;
    }

    /// <summary>
    /// Persists the transcript so <see cref="Screenshots"/> can render a real
    /// conversation rather than an invented one.
    /// </summary>
    private static void SaveTranscript(IReadOnlyList<ChatEntry> transcript, AppSettings settings)
    {
        var payload = new
        {
            provider = settings.Active.DisplayName,
            model = settings.Active.Model,
            captured = DateTimeOffset.Now,
            entries = transcript.Select(e => new
            {
                role = e.Role,
                text = e.Text,
                tools = e.ToolsUsed,
            }),
        };

        try
        {
            File.WriteAllText(
                TranscriptPath,
                JsonSerializer.Serialize(payload, new JsonSerializerOptions { WriteIndented = true }));
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"Could not save the transcript: {ex.Message}");
        }
    }

    /// <summary>Reads a saved transcript back, if one exists.</summary>
    public static (string Provider, string Model, List<ChatEntry> Entries)? LoadTranscript()
    {
        if (!File.Exists(TranscriptPath)) return null;

        try
        {
            using var document = JsonDocument.Parse(File.ReadAllText(TranscriptPath));
            var root = document.RootElement;

            var entries = new List<ChatEntry>();
            foreach (var element in root.GetProperty("entries").EnumerateArray())
            {
                var entry = new ChatEntry
                {
                    Role = element.GetProperty("role").GetString() ?? "note",
                    Text = element.GetProperty("text").GetString() ?? "",
                };
                if (element.TryGetProperty("tools", out var tools))
                {
                    foreach (var tool in tools.EnumerateArray())
                    {
                        if (tool.GetString() is { } name) entry.ToolsUsed.Add(name);
                    }
                }
                entries.Add(entry);
            }

            return (
                root.GetProperty("provider").GetString() ?? "",
                root.GetProperty("model").GetString() ?? "",
                entries);
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"Could not read the transcript: {ex.Message}");
            return null;
        }
    }

    private static bool Apply(AppSettings settings, string key, string value)
    {
        // Provider-scoped keys look like `OpenAI.ApiKey`.
        var dot = key.IndexOf('.');
        if (dot > 0 && Enum.TryParse<LlmProvider>(key[..dot], ignoreCase: true, out var provider))
        {
            var field = key[(dot + 1)..];
            var target = settings.Providers[provider];
            switch (field.ToLowerInvariant())
            {
                case "model": target.Model = value; return true;
                case "apikey": target.ApiKey = value; return true;
                case "endpoint": target.Endpoint = value; return true;
                default: return false;
            }
        }

        switch (key.ToLowerInvariant())
        {
            case "llm.provider":
                if (!Enum.TryParse<LlmProvider>(value, ignoreCase: true, out var active)) return false;
                settings.ActiveProvider = active;
                return true;
            case "llm.temperature":
                if (!double.TryParse(value, out var temperature)) return false;
                settings.Temperature = temperature;
                return true;
            case "llm.maxtokens":
                if (!int.TryParse(value, out var maxTokens)) return false;
                settings.MaxTokens = maxTokens;
                return true;
            case "llm.systemprompt": settings.SystemPrompt = value; return true;
            case "tavily.apikey": settings.TavilyApiKey = value; return true;
            case "toolchain.rustnetpath": settings.RustNetPath = value; return true;
            case "toolchain.dotnetpath": settings.DotnetPath = value; return true;
            case "workspace.lastproject": settings.LastProject = value; return true;
            default: return false;
        }
    }

    /// <summary>Never echo a secret back to the terminal in full.</summary>
    private static string Redact(string key, string value)
    {
        if (!key.Contains("key", StringComparison.OrdinalIgnoreCase)) return value;
        if (value.Length <= 8) return new string('•', value.Length);
        return $"{value[..4]}{new string('•', 8)}{value[^4..]}";
    }
}
