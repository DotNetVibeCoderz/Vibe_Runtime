namespace CodeGen.Models;

/// <summary>The LLM providers the assistant can talk to.</summary>
public enum LlmProvider
{
    OpenAI,
    Claude,
    Gemini,
    Ollama,
}

/// <summary>Connection details for one provider.</summary>
public sealed class ProviderSettings
{
    public required LlmProvider Provider { get; init; }
    public string Model { get; set; } = "";
    public string ApiKey { get; set; } = "";
    public string Endpoint { get; set; } = "";

    /// <summary>
    /// Whether the provider can be reached. Ollama runs locally and needs no
    /// key, so a missing key is only a problem for the hosted services.
    /// </summary>
    public bool IsConfigured =>
        !string.IsNullOrWhiteSpace(Model)
        && (Provider == LlmProvider.Ollama || !string.IsNullOrWhiteSpace(ApiKey));

    public string DisplayName => Provider switch
    {
        LlmProvider.OpenAI => "OpenAI",
        LlmProvider.Claude => "Claude",
        LlmProvider.Gemini => "Gemini",
        LlmProvider.Ollama => "Ollama",
        _ => Provider.ToString(),
    };
}

/// <summary>
/// Every setting CodeGen reads, mirroring the keys in <c>app.config</c>.
/// </summary>
public sealed class AppSettings
{
    public LlmProvider ActiveProvider { get; set; } = LlmProvider.Claude;
    public double Temperature { get; set; } = 0.2;
    public int MaxTokens { get; set; } = 8192;
    public string SystemPrompt { get; set; } = "";

    public Dictionary<LlmProvider, ProviderSettings> Providers { get; init; } = new();

    public string TavilyApiKey { get; set; } = "";
    public bool AllowShellCommands { get; set; } = true;
    public int MaxFileSizeKB { get; set; } = 512;

    public string RustNetPath { get; set; } = "";
    public string DotnetPath { get; set; } = "dotnet";

    public bool ShowLineNumbers { get; set; } = true;
    public string EditorFontFamily { get; set; } = "Cascadia Code, Consolas, monospace";
    public double EditorFontSize { get; set; } = 13;
    public int TabSize { get; set; } = 4;
    public bool WordWrap { get; set; }

    public double ChatPanelWidth { get; set; } = 380;
    public bool ChatPanelVisible { get; set; } = true;
    public double ExplorerWidth { get; set; } = 240;
    public bool ExplorerVisible { get; set; } = true;
    public double LogPanelHeight { get; set; } = 160;
    public bool LogPanelVisible { get; set; } = true;

    public string LastProject { get; set; } = "";

    public ProviderSettings Active => Providers[ActiveProvider];

    /// <summary>Models offered in the chat panel's picker for a provider.</summary>
    public static IReadOnlyList<string> KnownModels(LlmProvider provider) => provider switch
    {
        LlmProvider.OpenAI => ["gpt-4o", "gpt-4o-mini", "gpt-4.1", "o3-mini"],
        LlmProvider.Claude =>
            ["claude-opus-5", "claude-sonnet-5", "claude-haiku-4-5"],
        LlmProvider.Gemini => ["gemini-2.0-flash", "gemini-2.0-pro", "gemini-1.5-pro"],
        LlmProvider.Ollama => ["llama3.2", "qwen2.5-coder", "deepseek-coder-v2", "phi4"],
        _ => [],
    };
}
