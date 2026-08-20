using System.Runtime.CompilerServices;
using System.Text;
using System.Text.Json;
using Anthropic;
using Anthropic.Models.Messages;
using Microsoft.SemanticKernel;
using Microsoft.SemanticKernel.ChatCompletion;
using SKChatMessageContent = Microsoft.SemanticKernel.ChatMessageContent;
using SKAuthorRole = Microsoft.SemanticKernel.ChatCompletion.AuthorRole;

namespace CodeGen.Services;

/// <summary>
/// Claude as a Semantic Kernel chat service.
///
/// Semantic Kernel ships connectors for OpenAI-protocol endpoints, which covers
/// OpenAI, Gemini and Ollama. Anthropic speaks its own protocol, so this class
/// bridges the two: it converts Semantic Kernel's <see cref="ChatHistory"/> and
/// <see cref="KernelFunction"/>s into Anthropic messages and tools, runs the
/// tool-use loop against the official SDK, and hands the result back as a
/// Semantic Kernel message. Everything above this class treats Claude exactly
/// like the other providers.
/// </summary>
public sealed class AnthropicChatCompletionService : IChatCompletionService
{
    /// <summary>Stop the tool loop rather than spin if the model keeps calling.</summary>
    private const int MaxToolIterations = 12;

    private readonly AnthropicClient _client;
    private readonly string _model;
    private readonly int _maxTokens;
    private readonly Dictionary<string, object?> _attributes;

    public AnthropicChatCompletionService(string apiKey, string model, int maxTokens, string? endpoint = null)
    {
        _client = new AnthropicClient { ApiKey = apiKey };
        _model = model;
        _maxTokens = maxTokens <= 0 ? 8192 : maxTokens;
        _attributes = new Dictionary<string, object?>
        {
            ["ModelId"] = model,
            ["Provider"] = "Anthropic",
            ["Endpoint"] = endpoint ?? "https://api.anthropic.com",
        };
    }

    public IReadOnlyDictionary<string, object?> Attributes => _attributes;

    public async Task<IReadOnlyList<SKChatMessageContent>> GetChatMessageContentsAsync(
        ChatHistory chatHistory,
        PromptExecutionSettings? executionSettings = null,
        Kernel? kernel = null,
        CancellationToken cancellationToken = default)
    {
        var system = ExtractSystemPrompt(chatHistory);
        var messages = ToAnthropicMessages(chatHistory);
        var tools = kernel is null ? [] : BuildTools(kernel);

        var transcript = new StringBuilder();
        var toolTrace = new List<string>();

        for (var iteration = 0; iteration < MaxToolIterations; iteration++)
        {
            // `System` and `Tools` are init-only, so everything is set here.
            // An empty tool list is valid on the wire, and the kernel always
            // supplies at least one plugin, so no conditional is needed.
            var request = new MessageCreateParams
            {
                Model = _model,
                MaxTokens = _maxTokens,
                Messages = messages,
                // Adaptive thinking is the current API's mode; a fixed token
                // budget is rejected by Claude 4.6 and newer.
                Thinking = new ThinkingConfigAdaptive(),
                System = string.IsNullOrWhiteSpace(system)
                    ? "You are a careful coding assistant."
                    : system,
                Tools = tools,
            };

            var response = await _client.Messages.Create(request, cancellationToken: cancellationToken)
                .ConfigureAwait(false);

            // A safety refusal arrives as a 200 with no usable content.
            if (response.StopReason == StopReason.Refusal)
            {
                return
                [
                    new SKChatMessageContent(
                        SKAuthorRole.Assistant,
                        "Claude declined this request. Rephrasing it, or splitting it into smaller steps, usually helps."),
                ];
            }

            var assistantBlocks = new List<ContentBlockParam>();
            var toolResults = new List<ContentBlockParam>();

            foreach (var block in response.Content)
            {
                if (block.TryPickText(out TextBlock? text))
                {
                    transcript.Append(text.Text);
                    assistantBlocks.Add(new TextBlockParam { Text = text.Text });
                }
                else if (block.TryPickThinking(out ThinkingBlock? thinking))
                {
                    // The signature must survive the round trip untouched.
                    assistantBlocks.Add(new ThinkingBlockParam
                    {
                        Thinking = thinking.Thinking,
                        Signature = thinking.Signature,
                    });
                }
                else if (block.TryPickRedactedThinking(out RedactedThinkingBlock? redacted))
                {
                    assistantBlocks.Add(new RedactedThinkingBlockParam { Data = redacted.Data });
                }
                else if (block.TryPickToolUse(out ToolUseBlock? call))
                {
                    assistantBlocks.Add(new ToolUseBlockParam
                    {
                        ID = call.ID,
                        Name = call.Name,
                        Input = call.Input,
                    });

                    var (output, failed) = await InvokeToolAsync(kernel, call, cancellationToken)
                        .ConfigureAwait(false);
                    toolTrace.Add($"{call.Name}{(failed ? " (failed)" : "")}");

                    toolResults.Add(new ToolResultBlockParam
                    {
                        ToolUseID = call.ID,
                        Content = output,
                        IsError = failed,
                    });
                }
            }

            if (toolResults.Count == 0)
            {
                // No tools requested: this is the final answer.
                var content = new SKChatMessageContent(SKAuthorRole.Assistant, transcript.ToString());
                if (toolTrace.Count > 0)
                {
                    content.Metadata = new Dictionary<string, object?> { ["ToolsUsed"] = toolTrace };
                }
                return [content];
            }

            // Every tool_use needs a matching tool_result in a single user turn.
            messages = [.. messages,
                new MessageParam { Role = Role.Assistant, Content = assistantBlocks },
                new MessageParam { Role = Role.User, Content = toolResults }];
        }

        return
        [
            new SKChatMessageContent(
                SKAuthorRole.Assistant,
                transcript.Length > 0
                    ? transcript.ToString()
                    : $"Stopped after {MaxToolIterations} tool rounds without a final answer."),
        ];
    }

    /// <summary>
    /// Streaming falls back to a single non-streaming call.
    ///
    /// The tool loop needs the complete message before it can run a tool, so
    /// there is nothing meaningful to stream mid-loop. The chat panel shows a
    /// working indicator instead of partial text.
    /// </summary>
    public async IAsyncEnumerable<StreamingChatMessageContent> GetStreamingChatMessageContentsAsync(
        ChatHistory chatHistory,
        PromptExecutionSettings? executionSettings = null,
        Kernel? kernel = null,
        [EnumeratorCancellation] CancellationToken cancellationToken = default)
    {
        var results = await GetChatMessageContentsAsync(chatHistory, executionSettings, kernel, cancellationToken)
            .ConfigureAwait(false);
        foreach (var result in results)
        {
            yield return new StreamingChatMessageContent(result.Role, result.Content);
        }
    }

    // -- conversion ----------------------------------------------------------

    /// <summary>Anthropic takes the system prompt as a separate field.</summary>
    private static string ExtractSystemPrompt(ChatHistory history)
    {
        var parts = history
            .Where(m => m.Role == SKAuthorRole.System)
            .Select(m => m.Content)
            .Where(c => !string.IsNullOrWhiteSpace(c));
        return string.Join("\n\n", parts);
    }

    private static List<MessageParam> ToAnthropicMessages(ChatHistory history)
    {
        var messages = new List<MessageParam>();
        foreach (var message in history)
        {
            if (message.Role == SKAuthorRole.System) continue;
            if (string.IsNullOrWhiteSpace(message.Content)) continue;

            messages.Add(new MessageParam
            {
                Role = message.Role == SKAuthorRole.Assistant ? Role.Assistant : Role.User,
                Content = message.Content,
            });
        }

        // The API requires the conversation to open with a user turn.
        if (messages.Count == 0 || messages[0].Role != Role.User)
        {
            messages.Insert(0, new MessageParam { Role = Role.User, Content = "Hello." });
        }
        return messages;
    }

    /// <summary>Exposes every registered kernel function as an Anthropic tool.</summary>
    private static List<ToolUnion> BuildTools(Kernel kernel)
    {
        var tools = new List<ToolUnion>();
        foreach (var plugin in kernel.Plugins)
        {
            foreach (var function in plugin)
            {
                var properties = new Dictionary<string, JsonElement>();
                var required = new List<string>();

                foreach (var parameter in function.Metadata.Parameters)
                {
                    properties[parameter.Name] = JsonSerializer.SerializeToElement(new
                    {
                        type = JsonTypeFor(parameter.ParameterType),
                        description = parameter.Description ?? parameter.Name,
                    });
                    if (parameter.IsRequired) required.Add(parameter.Name);
                }

                tools.Add(new Tool
                {
                    // Anthropic tool names allow letters, digits, underscore
                    // and hyphen; Semantic Kernel joins plugin and function
                    // with a hyphen, which is already safe.
                    Name = $"{plugin.Name}-{function.Name}",
                    Description = function.Description,
                    // Target-typed: the schema type is nested and unexported.
                    InputSchema = new()
                    {
                        Properties = properties,
                        Required = required,
                    },
                });
            }
        }
        return tools;
    }

    // `Anthropic.Models.Messages` also defines a `Type`, so this one is spelled out.
    private static string JsonTypeFor(System.Type? type)
    {
        if (type is null) return "string";
        var underlying = Nullable.GetUnderlyingType(type) ?? type;
        if (underlying == typeof(bool)) return "boolean";
        if (underlying == typeof(int) || underlying == typeof(long)) return "integer";
        if (underlying == typeof(double) || underlying == typeof(float) || underlying == typeof(decimal))
        {
            return "number";
        }
        return "string";
    }

    /// <summary>Runs one tool call through the kernel.</summary>
    private static async Task<(string Output, bool Failed)> InvokeToolAsync(
        Kernel? kernel,
        ToolUseBlock call,
        CancellationToken cancellationToken)
    {
        if (kernel is null)
        {
            return ("No tools are available in this session.", true);
        }

        var separator = call.Name.IndexOf('-');
        if (separator <= 0)
        {
            return ($"Unknown tool '{call.Name}'.", true);
        }
        var pluginName = call.Name[..separator];
        var functionName = call.Name[(separator + 1)..];

        if (!kernel.Plugins.TryGetFunction(pluginName, functionName, out var function))
        {
            return ($"Unknown tool '{call.Name}'.", true);
        }

        var arguments = new KernelArguments();
        try
        {
            // Tool input arrives as JSON; parse it rather than string-matching.
            var input = JsonSerializer.Deserialize<Dictionary<string, JsonElement>>(call.Input.ToString() ?? "{}");
            if (input is not null)
            {
                foreach (var (key, value) in input)
                {
                    arguments[key] = value.ValueKind switch
                    {
                        JsonValueKind.String => value.GetString(),
                        JsonValueKind.Number => value.TryGetInt64(out var i) ? i : value.GetDouble(),
                        JsonValueKind.True => true,
                        JsonValueKind.False => false,
                        JsonValueKind.Null => null,
                        _ => value.ToString(),
                    };
                }
            }
        }
        catch (JsonException ex)
        {
            return ($"Could not read the arguments for '{call.Name}': {ex.Message}", true);
        }

        try
        {
            var result = await function.InvokeAsync(kernel, arguments, cancellationToken).ConfigureAwait(false);
            return (result.ToString(), false);
        }
        catch (Exception ex)
        {
            // Report the failure to the model so it can adjust, rather than
            // aborting the conversation.
            return ($"{call.Name} failed: {ex.Message}", true);
        }
    }
}
