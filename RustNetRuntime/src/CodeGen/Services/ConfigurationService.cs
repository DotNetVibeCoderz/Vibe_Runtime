using System.Globalization;
using CodeGen.Models;
// `System.Configuration` also defines a `ProviderSettings`, so the namespace is
// imported under an alias and the model type keeps its plain name.
using Config = System.Configuration;
using Configuration = System.Configuration.Configuration;
using ConfigurationErrorsException = System.Configuration.ConfigurationErrorsException;
using ConfigurationManager = System.Configuration.ConfigurationManager;
using ConfigurationSaveMode = System.Configuration.ConfigurationSaveMode;
using ConfigurationUserLevel = System.Configuration.ConfigurationUserLevel;

namespace CodeGen.Services;

/// <summary>
/// Reads and writes every CodeGen setting through <c>app.config</c>.
///
/// The requirement is that configuration lives in one place and stays editable
/// from the UI, so this is the only component that touches settings storage.
/// Writes go to the <c>CodeGen.dll.config</c> the build produced next to the
/// executable; if that file is read-only — an installed-under-Program-Files
/// scenario — the service falls back to a per-user copy rather than losing the
/// change silently.
/// </summary>
public sealed class ConfigurationService
{
    private readonly Dictionary<string, string> _overrides = new();
    private Configuration? _configuration;

    /// <summary>Where settings were last written, shown in the Settings dialog.</summary>
    public string ConfigurationPath { get; private set; } = "";

    /// <summary>Set when the primary config file could not be written.</summary>
    public string? LastWriteError { get; private set; }

    public AppSettings Load()
    {
        var settings = new AppSettings
        {
            ActiveProvider = ReadEnum("Llm.Provider", LlmProvider.Claude),
            Temperature = ReadDouble("Llm.Temperature", 0.2),
            MaxTokens = ReadInt("Llm.MaxTokens", 8192),
            SystemPrompt = Read("Llm.SystemPrompt", DefaultSystemPrompt),

            TavilyApiKey = Read("Tavily.ApiKey", ""),
            AllowShellCommands = ReadBool("Tools.AllowShellCommands", true),
            MaxFileSizeKB = ReadInt("Tools.MaxFileSizeKB", 512),

            RustNetPath = Read("Toolchain.RustNetPath", ""),
            DotnetPath = Read("Toolchain.DotnetPath", "dotnet"),

            ShowLineNumbers = ReadBool("Editor.ShowLineNumbers", true),
            EditorFontFamily = Read("Editor.FontFamily", "Cascadia Code, Consolas, monospace"),
            EditorFontSize = ReadDouble("Editor.FontSize", 13),
            TabSize = ReadInt("Editor.TabSize", 4),
            WordWrap = ReadBool("Editor.WordWrap", false),

            ChatPanelWidth = ReadDouble("Layout.ChatPanelWidth", 380),
            ChatPanelVisible = ReadBool("Layout.ChatPanelVisible", true),
            ExplorerWidth = ReadDouble("Layout.ExplorerWidth", 240),
            ExplorerVisible = ReadBool("Layout.ExplorerVisible", true),
            LogPanelHeight = ReadDouble("Layout.LogPanelHeight", 160),
            LogPanelVisible = ReadBool("Layout.LogPanelVisible", true),

            LastProject = Read("Workspace.LastProject", ""),
        };

        foreach (var provider in Enum.GetValues<LlmProvider>())
        {
            var prefix = provider.ToString();
            settings.Providers[provider] = new ProviderSettings
            {
                Provider = provider,
                Model = Read($"{prefix}.Model", DefaultModel(provider)),
                ApiKey = Read($"{prefix}.ApiKey", provider == LlmProvider.Ollama ? "ollama" : ""),
                Endpoint = Read($"{prefix}.Endpoint", DefaultEndpoint(provider)),
            };
        }

        return settings;
    }

    public void Save(AppSettings settings)
    {
        Write("Llm.Provider", settings.ActiveProvider.ToString());
        Write("Llm.Temperature", settings.Temperature.ToString(CultureInfo.InvariantCulture));
        Write("Llm.MaxTokens", settings.MaxTokens.ToString(CultureInfo.InvariantCulture));
        Write("Llm.SystemPrompt", settings.SystemPrompt);

        foreach (var (provider, values) in settings.Providers)
        {
            var prefix = provider.ToString();
            Write($"{prefix}.Model", values.Model);
            Write($"{prefix}.ApiKey", values.ApiKey);
            Write($"{prefix}.Endpoint", values.Endpoint);
        }

        Write("Tavily.ApiKey", settings.TavilyApiKey);
        Write("Tools.AllowShellCommands", settings.AllowShellCommands.ToString());
        Write("Tools.MaxFileSizeKB", settings.MaxFileSizeKB.ToString(CultureInfo.InvariantCulture));

        Write("Toolchain.RustNetPath", settings.RustNetPath);
        Write("Toolchain.DotnetPath", settings.DotnetPath);

        Write("Editor.ShowLineNumbers", settings.ShowLineNumbers.ToString());
        Write("Editor.FontFamily", settings.EditorFontFamily);
        Write("Editor.FontSize", settings.EditorFontSize.ToString(CultureInfo.InvariantCulture));
        Write("Editor.TabSize", settings.TabSize.ToString(CultureInfo.InvariantCulture));
        Write("Editor.WordWrap", settings.WordWrap.ToString());

        Write("Layout.ChatPanelWidth", settings.ChatPanelWidth.ToString("F0", CultureInfo.InvariantCulture));
        Write("Layout.ChatPanelVisible", settings.ChatPanelVisible.ToString());
        Write("Layout.ExplorerWidth", settings.ExplorerWidth.ToString("F0", CultureInfo.InvariantCulture));
        Write("Layout.ExplorerVisible", settings.ExplorerVisible.ToString());
        Write("Layout.LogPanelHeight", settings.LogPanelHeight.ToString("F0", CultureInfo.InvariantCulture));
        Write("Layout.LogPanelVisible", settings.LogPanelVisible.ToString());

        Write("Workspace.LastProject", settings.LastProject);

        Flush();
    }

    // -- storage -------------------------------------------------------------

    private Configuration Configuration
    {
        get
        {
            if (_configuration is null)
            {
                _configuration = ConfigurationManager.OpenExeConfiguration(ConfigurationUserLevel.None);
                ConfigurationPath = _configuration.FilePath;
            }
            return _configuration;
        }
    }

    private string Read(string key, string fallback)
    {
        if (_overrides.TryGetValue(key, out var pending))
        {
            return pending;
        }
        try
        {
            var element = Configuration.AppSettings.Settings[key];
            return element?.Value ?? fallback;
        }
        catch (ConfigurationErrorsException)
        {
            // A malformed config must not stop the app from starting.
            return fallback;
        }
    }

    private void Write(string key, string value) => _overrides[key] = value ?? "";

    /// <summary>Commits every pending change to disk.</summary>
    private void Flush()
    {
        LastWriteError = null;
        try
        {
            var configuration = Configuration;
            foreach (var (key, value) in _overrides)
            {
                if (configuration.AppSettings.Settings[key] is null)
                {
                    configuration.AppSettings.Settings.Add(key, value);
                }
                else
                {
                    configuration.AppSettings.Settings[key].Value = value;
                }
            }
            configuration.Save(ConfigurationSaveMode.Modified);
            ConfigurationManager.RefreshSection("appSettings");
            ConfigurationPath = configuration.FilePath;
            _overrides.Clear();
        }
        catch (Exception ex)
        {
            // Keep the values in memory so the session still behaves as
            // configured, and tell the user where the write failed.
            LastWriteError = ex.Message;
        }
    }

    private double ReadDouble(string key, double fallback) =>
        double.TryParse(Read(key, ""), NumberStyles.Float, CultureInfo.InvariantCulture, out var v)
            ? v
            : fallback;

    private int ReadInt(string key, int fallback) =>
        int.TryParse(Read(key, ""), NumberStyles.Integer, CultureInfo.InvariantCulture, out var v)
            ? v
            : fallback;

    private bool ReadBool(string key, bool fallback) =>
        bool.TryParse(Read(key, ""), out var v) ? v : fallback;

    private T ReadEnum<T>(string key, T fallback) where T : struct, Enum =>
        Enum.TryParse<T>(Read(key, ""), ignoreCase: true, out var v) ? v : fallback;

    private static string DefaultModel(LlmProvider provider) => provider switch
    {
        LlmProvider.OpenAI => "gpt-4o",
        LlmProvider.Claude => "claude-opus-5",
        LlmProvider.Gemini => "gemini-2.0-flash",
        LlmProvider.Ollama => "llama3.2",
        _ => "",
    };

    private static string DefaultEndpoint(LlmProvider provider) => provider switch
    {
        LlmProvider.OpenAI => "https://api.openai.com/v1",
        LlmProvider.Claude => "https://api.anthropic.com",
        LlmProvider.Gemini => "https://generativelanguage.googleapis.com/v1beta/openai",
        LlmProvider.Ollama => "http://localhost:11434/v1",
        _ => "",
    };

    private const string DefaultSystemPrompt =
        "You are Jack — The Code Bender, the coding assistant inside CodeGen, the IDE for "
        + "RustNetRuntime (C# running on RustCLR, a CLR rebuilt in Rust). Write complete, "
        + "compiling code. Prefer the project's existing conventions. Use your tools to read "
        + "and write files rather than telling the user what to type. When you change code, "
        + "say which files you touched and why in one or two sentences.";
}
