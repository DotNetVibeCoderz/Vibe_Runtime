using System;
using System.Collections.Generic;
using System.Configuration;
using System.Globalization;
using System.IO;

namespace RustNet.Designer.Assistant;

/// <summary>The chat providers the assistant can be pointed at.</summary>
public enum AiProvider
{
    OpenAI,
    Anthropic,
    Gemini,
    Ollama,
}

/// <summary>
/// Everything the assistant reads out of <c>app.config</c>: persona, sampling
/// knobs, the selected provider and its model, tool keys.
///
/// Values are read once at startup and written back by <see cref="Save"/> when
/// the chat panel changes them, so app.config stays the source of truth rather
/// than a one-time seed. API keys may be given literally or as
/// <c>${ENV_VAR}</c>, which is resolved from the environment — that keeps real
/// keys out of a file that lives in the repo.
/// </summary>
public sealed class AssistantOptions
{
    public bool Enabled { get; set; } = true;
    public AiProvider Provider { get; set; } = AiProvider.Anthropic;
    public bool Streaming { get; set; } = true;
    public double Temperature { get; set; } = 0.35;
    public double TopP { get; set; } = 0.95;
    public int MaxTokens { get; set; } = 8192;
    public int MaxToolIterations { get; set; } = 12;
    public int HistoryTurns { get; set; } = 40;
    public string Persona { get; set; } = "";

    public string DataDirectory { get; set; } = "";
    public string WorkspaceRoot { get; set; } = "";

    public ProviderOptions OpenAI { get; } = new("OpenAI");
    public ProviderOptions Anthropic { get; } = new("Anthropic");
    public ProviderOptions Gemini { get; } = new("Gemini");
    public ProviderOptions Ollama { get; } = new("Ollama");

    public bool ToolsEnabled { get; set; } = true;
    public string TavilyApiKey { get; set; } = "";
    public int TavilyMaxResults { get; set; } = 5;
    public int HttpTimeoutSeconds { get; set; } = 30;
    public int HttpMaxChars { get; set; } = 60000;

    public long MaxImageBytes { get; set; } = 5_000_000;
    public int MaxDocumentChars { get; set; } = 40_000;

    /// <summary>Per-provider endpoint/key/model, and the model picker's choices.</summary>
    public sealed class ProviderOptions
    {
        public ProviderOptions(string name) => Name = name;

        public string Name { get; }
        public string ApiKey { get; set; } = "";
        public string Model { get; set; } = "";
        public string Endpoint { get; set; } = "";
        public List<string> Models { get; } = new();
    }

    public ProviderOptions For(AiProvider provider) => provider switch
    {
        AiProvider.OpenAI => OpenAI,
        AiProvider.Anthropic => Anthropic,
        AiProvider.Gemini => Gemini,
        AiProvider.Ollama => Ollama,
        _ => throw new ArgumentOutOfRangeException(nameof(provider)),
    };

    public ProviderOptions Current => For(Provider);

    /// <summary>
    /// True when the selected provider has what it needs to talk to a model.
    /// Ollama is local and keyless, so it only needs an endpoint.
    /// </summary>
    public bool IsProviderConfigured(AiProvider provider)
    {
        ProviderOptions p = For(provider);
        if (p.Model.Length == 0)
        {
            return false;
        }
        return provider == AiProvider.Ollama ? p.Endpoint.Length > 0 : p.ApiKey.Length > 0;
    }

    // ---- load --------------------------------------------------------

    public static AssistantOptions Load()
    {
        var o = new AssistantOptions();
        o.Enabled = Bool("Assistant.Enabled", true);
        o.Provider = ParseProvider(Str("Assistant.Provider"), AiProvider.Anthropic);
        o.Streaming = Bool("Assistant.Streaming", true);
        o.Temperature = Num("Assistant.Temperature", 0.35);
        o.TopP = Num("Assistant.TopP", 0.95);
        o.MaxTokens = Int("Assistant.MaxTokens", 8192);
        o.MaxToolIterations = Int("Assistant.MaxToolIterations", 12);
        o.HistoryTurns = Int("Assistant.HistoryTurns", 40);
        o.Persona = Unescape(Str("Assistant.Persona"));
        o.DataDirectory = Str("Assistant.DataDirectory");
        o.WorkspaceRoot = Str("Assistant.WorkspaceRoot");

        LoadProvider(o.OpenAI);
        LoadProvider(o.Anthropic);
        LoadProvider(o.Gemini);
        LoadProvider(o.Ollama);

        o.ToolsEnabled = Bool("Tools.Enabled", true);
        o.TavilyApiKey = Resolve(Str("Tools.Tavily.ApiKey"));
        o.TavilyMaxResults = Int("Tools.Tavily.MaxResults", 5);
        o.HttpTimeoutSeconds = Int("Tools.Http.TimeoutSeconds", 30);
        o.HttpMaxChars = Int("Tools.Http.MaxChars", 60000);

        o.MaxImageBytes = Int("Attachments.MaxImageBytes", 5_000_000);
        o.MaxDocumentChars = Int("Attachments.MaxDocumentChars", 40_000);

        if (o.Persona.Length == 0)
        {
            o.Persona = PromptLibrary.DefaultPersona;
        }
        if (o.DataDirectory.Length == 0)
        {
            o.DataDirectory = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
                "RustNet", "Designer", "assistant");
        }
        if (o.WorkspaceRoot.Length == 0)
        {
            o.WorkspaceRoot = DetectWorkspaceRoot();
        }
        return o;
    }

    private static void LoadProvider(ProviderOptions p)
    {
        p.ApiKey = Resolve(Str($"Assistant.{p.Name}.ApiKey"));
        p.Model = Str($"Assistant.{p.Name}.Model");
        p.Endpoint = Str($"Assistant.{p.Name}.Endpoint");
        p.Models.Clear();
        foreach (string m in Str($"Assistant.{p.Name}.Models").Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
        {
            p.Models.Add(m);
        }
        if (p.Model.Length > 0 && !p.Models.Contains(p.Model))
        {
            p.Models.Insert(0, p.Model);
        }
    }

    /// <summary>
    /// Find the RustNet checkout so the docs/template functions have something
    /// to read. $(RUSTNET_SDK) is what the templates already use; failing that,
    /// walk up from the running binary looking for the marker files.
    /// </summary>
    private static string DetectWorkspaceRoot()
    {
        string? sdk = Environment.GetEnvironmentVariable("RUSTNET_SDK");
        if (!string.IsNullOrWhiteSpace(sdk) && Directory.Exists(sdk))
        {
            return sdk;
        }
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir != null)
        {
            if (File.Exists(Path.Combine(dir.FullName, "dotnet", "RustNet.slnx"))
                || Directory.Exists(Path.Combine(dir.FullName, "templates")) && Directory.Exists(Path.Combine(dir.FullName, "runtime")))
            {
                return dir.FullName;
            }
            dir = dir.Parent;
        }
        return Directory.GetCurrentDirectory();
    }

    // ---- save --------------------------------------------------------

    /// <summary>
    /// Persist the knobs the chat panel can change. Keys are left untouched
    /// unless they were entered literally, so a <c>${ENV_VAR}</c> placeholder
    /// is never overwritten with the value it resolved to.
    /// </summary>
    public void Save()
    {
        try
        {
            Configuration cfg = ConfigurationManager.OpenExeConfiguration(ConfigurationUserLevel.None);
            Set(cfg, "Assistant.Provider", Provider.ToString());
            Set(cfg, "Assistant.Streaming", Streaming ? "true" : "false");
            Set(cfg, "Assistant.Temperature", Temperature.ToString("0.###", CultureInfo.InvariantCulture));
            Set(cfg, "Assistant.TopP", TopP.ToString("0.###", CultureInfo.InvariantCulture));
            Set(cfg, "Assistant.MaxTokens", MaxTokens.ToString(CultureInfo.InvariantCulture));
            Set(cfg, "Assistant.MaxToolIterations", MaxToolIterations.ToString(CultureInfo.InvariantCulture));
            Set(cfg, "Assistant.HistoryTurns", HistoryTurns.ToString(CultureInfo.InvariantCulture));
            Set(cfg, "Assistant.Persona", Escape(Persona));
            Set(cfg, "Tools.Enabled", ToolsEnabled ? "true" : "false");
            foreach (AiProvider p in new[] { AiProvider.OpenAI, AiProvider.Anthropic, AiProvider.Gemini, AiProvider.Ollama })
            {
                Set(cfg, $"Assistant.{For(p).Name}.Model", For(p).Model);
            }
            cfg.Save(ConfigurationSaveMode.Modified);
            ConfigurationManager.RefreshSection("appSettings");
        }
        catch (Exception ex)
        {
            // A read-only install directory is not a reason to lose the turn;
            // the in-memory options stay applied for this run.
            System.Diagnostics.Debug.WriteLine("assistant settings not saved: " + ex.Message);
        }
    }

    private static void Set(Configuration cfg, string key, string value)
    {
        KeyValueConfigurationElement? existing = cfg.AppSettings.Settings[key];
        if (existing == null)
        {
            cfg.AppSettings.Settings.Add(key, value);
        }
        else
        {
            existing.Value = value;
        }
    }

    // ---- primitives --------------------------------------------------

    private static string Str(string key) => ConfigurationManager.AppSettings[key]?.Trim() ?? "";

    private static bool Bool(string key, bool fallback)
        => bool.TryParse(Str(key), out bool v) ? v : fallback;

    private static int Int(string key, int fallback)
        => int.TryParse(Str(key), NumberStyles.Integer, CultureInfo.InvariantCulture, out int v) ? v : fallback;

    private static double Num(string key, double fallback)
        => double.TryParse(Str(key), NumberStyles.Float, CultureInfo.InvariantCulture, out double v) ? v : fallback;

    public static AiProvider ParseProvider(string s, AiProvider fallback)
        => Enum.TryParse(s, ignoreCase: true, out AiProvider v) ? v : fallback;

    /// <summary>Expand a <c>${ENV_VAR}</c> placeholder; anything else is literal.</summary>
    private static string Resolve(string value)
    {
        if (value.Length > 3 && value.StartsWith("${", StringComparison.Ordinal) && value.EndsWith("}", StringComparison.Ordinal))
        {
            string name = value.Substring(2, value.Length - 3);
            return Environment.GetEnvironmentVariable(name)?.Trim() ?? "";
        }
        return value;
    }

    // XML attribute-value normalisation collapses real newlines, so the
    // persona is stored with \n escapes.
    private static string Unescape(string s) => s.Replace("\\n", "\n").Replace("\\t", "\t");
    private static string Escape(string s) => s.Replace("\r\n", "\n").Replace("\n", "\\n");
}
