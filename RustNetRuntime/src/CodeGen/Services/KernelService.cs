using System.ClientModel;
using CodeGen.Models;
using CodeGen.Plugins;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.SemanticKernel;
using Microsoft.SemanticKernel.ChatCompletion;
using Microsoft.SemanticKernel.Connectors.OpenAI;
using OpenAI;

namespace CodeGen.Services;

/// <summary>One turn of the conversation, as the chat panel shows it.</summary>
public sealed class ChatEntry
{
    public required string Role { get; init; }
    public required string Text { get; set; }
    public DateTimeOffset At { get; init; } = DateTimeOffset.Now;
    public List<string> Attachments { get; init; } = [];
    public List<string> ToolsUsed { get; init; } = [];

    public bool IsUser => Role == "user";
    public bool IsAssistant => Role == "assistant";
    public bool IsSystemNote => Role == "note";
    public string Speaker => Role switch
    {
        "user" => "You",
        "assistant" => "Jack",
        _ => "CodeGen",
    };
    public string Timestamp => At.ToString("HH:mm");
    public bool HasTools => ToolsUsed.Count > 0;
    public string ToolSummary => ToolsUsed.Count == 0 ? "" : string.Join(" · ", ToolsUsed);

    /// <summary>
    /// The text as the panel shows it.
    ///
    /// Models reply in Markdown. The panel is not a Markdown renderer, and
    /// leaving `**` and backticks on screen reads as noise, so the emphasis
    /// markers are dropped while the words and structure are left untouched.
    /// <see cref="Text"/> keeps the original.
    /// </summary>
    public string Display => StripEmphasis(Text);

    private static string StripEmphasis(string markdown)
    {
        if (string.IsNullOrEmpty(markdown)) return markdown;

        var builder = new System.Text.StringBuilder(markdown.Length);
        for (var i = 0; i < markdown.Length; i++)
        {
            var c = markdown[i];
            if (c == '`')
            {
                continue;
            }
            if (c == '*')
            {
                // Drop `**` and `*` used for emphasis, but keep a bullet: a
                // `*` that opens a line and is followed by a space is a list.
                var startsLine = i == 0 || markdown[i - 1] == (char)10;
                var isBullet = startsLine && i + 1 < markdown.Length && markdown[i + 1] == ' ';
                if (isBullet)
                {
                    builder.Append('•');
                    continue;
                }
                continue;
            }
            builder.Append(c);
        }
        return builder.ToString();
    }
}

/// <summary>
/// Builds the Semantic Kernel the assistant runs on, and holds the thread.
///
/// Three of the four providers speak the OpenAI protocol — OpenAI itself,
/// Gemini through its compatibility endpoint, and Ollama — so they share one
/// connector and differ only by base address. Claude speaks its own protocol
/// and is bridged by <see cref="AnthropicChatCompletionService"/>. Above this
/// class, all four look identical.
/// </summary>
public sealed class KernelService(
    ProjectService projects,
    BuildService builds,
    ProcessRunner runner,
    Action<string> log)
{
    private readonly HttpClient _http = new()
    {
        Timeout = TimeSpan.FromSeconds(60),
        DefaultRequestHeaders = { { "User-Agent", "CodeGen/0.1 (RustNetRuntime)" } },
    };

    private Kernel? _kernel;
    private IChatCompletionService? _chat;
    private ChatHistory _history = [];
    private string _signature = "";

    /// <summary>The conversation as the UI shows it.</summary>
    public List<ChatEntry> Transcript { get; } = [];

    /// <summary>Set when the last rebuild failed, for the UI to surface.</summary>
    public string? ConfigurationProblem { get; private set; }

    public bool IsReady => _chat is not null;

    /// <summary>
    /// Rebuilds the kernel when the provider, model or key changed.
    /// Cheap to call on every send.
    /// </summary>
    public void EnsureConfigured(AppSettings settings)
    {
        var active = settings.Active;
        var signature = $"{active.Provider}|{active.Model}|{active.Endpoint}|{active.ApiKey.Length}|{settings.MaxTokens}";
        if (_kernel is not null && signature == _signature) return;

        ConfigurationProblem = null;
        try
        {
            _kernel = BuildKernel(settings);
            _chat = _kernel.GetRequiredService<IChatCompletionService>();
            _signature = signature;
            ResetThread(settings);
            log($"Jack is using {active.DisplayName} · {active.Model}");
        }
        catch (Exception ex)
        {
            _kernel = null;
            _chat = null;
            _signature = "";
            ConfigurationProblem = ex.Message;
        }
    }

    private Kernel BuildKernel(AppSettings settings)
    {
        var active = settings.Active;
        if (string.IsNullOrWhiteSpace(active.Model))
        {
            throw new InvalidOperationException($"No model is set for {active.DisplayName}.");
        }
        if (!active.IsConfigured)
        {
            throw new InvalidOperationException(
                $"{active.DisplayName} needs an API key. Add it under Settings → Providers.");
        }

        var builder = Kernel.CreateBuilder();

        if (active.Provider == LlmProvider.Claude)
        {
            builder.Services.AddSingleton<IChatCompletionService>(
                new AnthropicChatCompletionService(
                    active.ApiKey,
                    active.Model,
                    settings.MaxTokens,
                    active.Endpoint));
        }
        else
        {
            // Gemini and Ollama both expose OpenAI-compatible endpoints, so one
            // connector covers three providers.
            var options = new OpenAIClientOptions();
            if (!string.IsNullOrWhiteSpace(active.Endpoint))
            {
                options.Endpoint = new Uri(active.Endpoint);
            }
            var client = new OpenAIClient(new ApiKeyCredential(active.ApiKey), options);
            builder.AddOpenAIChatCompletion(active.Model, client);
        }

        var kernel = builder.Build();

        kernel.Plugins.AddFromObject(new WorkspacePlugin(projects, settings, log), "workspace");
        kernel.Plugins.AddFromObject(new ToolchainPlugin(projects, builds, runner, settings, log), "toolchain");
        kernel.Plugins.AddFromObject(new WebPlugin(settings, _http, log), "web");
        kernel.Plugins.AddFromObject(new UtilityPlugin(log), "utility");

        return kernel;
    }

    /// <summary>Clears the thread, keeping the system prompt.</summary>
    public void ResetThread(AppSettings settings)
    {
        _history = [];
        var prompt = string.IsNullOrWhiteSpace(settings.SystemPrompt)
            ? "You are Jack — The Code Bender."
            : settings.SystemPrompt;

        var project = projects.CurrentProjectPath is null
            ? "No project is open yet."
            : $"The open project is '{projects.CurrentProjectName}' at {projects.CurrentProjectPath}.";

        _history.AddSystemMessage($"{prompt}\n\n{project}");
        Transcript.Clear();
    }

    /// <summary>Sends a message and returns Jack's reply.</summary>
    public async Task<ChatEntry> SendAsync(
        AppSettings settings,
        string message,
        IReadOnlyList<string>? attachments = null,
        CancellationToken cancellationToken = default)
    {
        EnsureConfigured(settings);

        if (_chat is null || _kernel is null)
        {
            var problem = ConfigurationProblem ?? "The assistant is not configured.";
            var note = new ChatEntry { Role = "note", Text = problem };
            Transcript.Add(note);
            return note;
        }

        var outgoing = new ChatEntry { Role = "user", Text = message };
        if (attachments is not null) outgoing.Attachments.AddRange(attachments);
        Transcript.Add(outgoing);

        // Images ride along as a note in the prompt: the tools read files from
        // disk, so a path is more useful to Jack than an inlined bitmap.
        var prompt = message;
        if (attachments is { Count: > 0 })
        {
            prompt += "\n\nAttached files:\n" + string.Join('\n', attachments.Select(a => $"- {a}"));
        }
        _history.AddUserMessage(prompt);

        var settingsForTurn = BuildExecutionSettings(settings);

        try
        {
            var replies = await _chat
                .GetChatMessageContentsAsync(_history, settingsForTurn, _kernel, cancellationToken)
                .ConfigureAwait(false);

            var text = string.Join("\n", replies.Select(r => r.Content).Where(c => !string.IsNullOrEmpty(c)));
            if (string.IsNullOrWhiteSpace(text)) text = "(no reply)";

            _history.AddAssistantMessage(text);

            var reply = new ChatEntry { Role = "assistant", Text = text };
            if (replies.Count > 0
                && replies[0].Metadata is { } metadata
                && metadata.TryGetValue("ToolsUsed", out var used)
                && used is List<string> tools)
            {
                reply.ToolsUsed.AddRange(tools);
            }
            Transcript.Add(reply);
            return reply;
        }
        catch (OperationCanceledException)
        {
            var note = new ChatEntry { Role = "note", Text = "Cancelled." };
            Transcript.Add(note);
            return note;
        }
        catch (Exception ex)
        {
            // Keep the thread usable: report the failure and drop the turn that
            // caused it, so the next message is not sent into a broken history.
            if (_history.Count > 0) _history.RemoveAt(_history.Count - 1);
            var note = new ChatEntry { Role = "note", Text = $"{active(settings)} failed: {ex.Message}" };
            Transcript.Add(note);
            return note;
        }

        static string active(AppSettings s) => s.Active.DisplayName;
    }

    /// <summary>
    /// Per-turn generation settings.
    ///
    /// Claude 4.6 and newer reject <c>temperature</c> outright, so it is only
    /// sent to the OpenAI-protocol providers. The setting still applies there.
    /// </summary>
    private static PromptExecutionSettings? BuildExecutionSettings(AppSettings settings)
    {
        if (settings.ActiveProvider == LlmProvider.Claude) return null;

        return new OpenAIPromptExecutionSettings
        {
            Temperature = settings.Temperature,
            MaxTokens = settings.MaxTokens,
            // Let the connector run the tool loop for us.
            FunctionChoiceBehavior = FunctionChoiceBehavior.Auto(),
        };
    }

    /// <summary>Names of every tool the assistant can call, for the UI.</summary>
    public IReadOnlyList<string> AvailableTools()
    {
        if (_kernel is null) return [];
        return [.. _kernel.Plugins.SelectMany(p => p.Select(f => $"{p.Name}.{f.Name}"))];
    }
}
