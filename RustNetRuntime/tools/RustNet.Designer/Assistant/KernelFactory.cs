using System;
using System.Collections.Generic;
using Anthropic.SDK;
using Microsoft.Extensions.AI;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.SemanticKernel;
using Microsoft.SemanticKernel.ChatCompletion;
using Microsoft.SemanticKernel.Connectors.Google;
using Microsoft.SemanticKernel.Connectors.Ollama;
using Microsoft.SemanticKernel.Connectors.OpenAI;

namespace RustNet.Designer.Assistant;

/// <summary>
/// Builds the Semantic Kernel for the selected provider and turns the
/// assistant's settings into that provider's execution settings.
///
/// OpenAI, Gemini and Ollama use their first-party SK connectors. Anthropic has
/// none, so Anthropic.SDK's <see cref="IChatClient"/> is bridged in with
/// <c>AsChatCompletionService()</c>; <c>UseFunctionInvocation()</c> puts the
/// tool loop in front of it so kernel functions work there too.
/// </summary>
public static class KernelFactory
{
    /// <summary>
    /// Construct a kernel for <paramref name="options"/>' current provider with
    /// <paramref name="plugins"/> registered. Throws
    /// <see cref="InvalidOperationException"/> with a fixable message when the
    /// provider is not configured — the panel shows it verbatim.
    /// </summary>
    public static Kernel Create(AssistantOptions options, IEnumerable<KernelPlugin> plugins, ToolTap tap)
    {
        AssistantOptions.ProviderOptions p = options.Current;
        if (p.Model.Length == 0)
        {
            throw new InvalidOperationException(
                $"No model selected for {options.Provider}. Set Assistant.{p.Name}.Model in app.config.");
        }

        IKernelBuilder builder = Kernel.CreateBuilder();

        switch (options.Provider)
        {
            case AiProvider.OpenAI:
                RequireKey(p, "OPENAI_API_KEY");
                if (p.Endpoint.Length > 0)
                {
                    builder.AddOpenAIChatCompletion(p.Model, new Uri(p.Endpoint), p.ApiKey);
                }
                else
                {
                    builder.AddOpenAIChatCompletion(p.Model, p.ApiKey);
                }
                break;

            case AiProvider.Gemini:
                RequireKey(p, "GEMINI_API_KEY");
                builder.AddGoogleAIGeminiChatCompletion(p.Model, p.ApiKey);
                break;

            case AiProvider.Ollama:
                builder.AddOllamaChatCompletion(
                    p.Model,
                    new Uri(p.Endpoint.Length > 0 ? p.Endpoint : "http://localhost:11434"));
                break;

            case AiProvider.Anthropic:
            {
                RequireKey(p, "ANTHROPIC_API_KEY");
                var anthropic = new AnthropicClient(new APIAuthentication(p.ApiKey));
                IChatClient chat = anthropic.Messages;
                // The Anthropic endpoint reports tool calls but does not run
                // them; this middleware does, which is what the SK connectors
                // do internally for the other providers.
                IChatClient piped = new ToolTapChatClient(chat, tap)
                    .AsBuilder()
                    .UseFunctionInvocation()
                    .Build();
                builder.Services.AddSingleton<IChatCompletionService>(piped.AsChatCompletionService());
                break;
            }

            default:
                throw new InvalidOperationException("Unknown provider " + options.Provider);
        }

        Kernel kernel = builder.Build();
        foreach (KernelPlugin plugin in plugins)
        {
            kernel.Plugins.Add(plugin);
        }
        return kernel;
    }

    private static void RequireKey(AssistantOptions.ProviderOptions p, string envVar)
    {
        if (p.ApiKey.Length == 0)
        {
            throw new InvalidOperationException(
                $"No API key for {p.Name}. Set Assistant.{p.Name}.ApiKey in app.config, "
                + $"or export {envVar} and leave the value as ${{{envVar}}}.");
        }
    }

    /// <summary>
    /// Sampling knobs in the shape each connector expects, with automatic
    /// function calling on when tools are enabled.
    /// </summary>
    public static PromptExecutionSettings CreateSettings(AssistantOptions options)
    {
        FunctionChoiceBehavior? tools = options.ToolsEnabled
            ? FunctionChoiceBehavior.Auto(autoInvoke: true,
                options: new FunctionChoiceBehaviorOptions { AllowConcurrentInvocation = false })
            : null;

        switch (options.Provider)
        {
            case AiProvider.OpenAI:
                return new OpenAIPromptExecutionSettings
                {
                    Temperature = options.Temperature,
                    TopP = options.TopP,
                    MaxTokens = options.MaxTokens,
                    FunctionChoiceBehavior = tools,
                };

            case AiProvider.Gemini:
                return new GeminiPromptExecutionSettings
                {
                    Temperature = options.Temperature,
                    TopP = options.TopP,
                    MaxTokens = options.MaxTokens,
                    FunctionChoiceBehavior = tools,
                };

            case AiProvider.Ollama:
                return new OllamaPromptExecutionSettings
                {
                    Temperature = (float)options.Temperature,
                    TopP = (float)options.TopP,
                    FunctionChoiceBehavior = tools,
                };

            case AiProvider.Anthropic:
            default:
                // The IChatClient bridge reads the model id and the standard
                // sampling keys off the base settings.
                return new PromptExecutionSettings
                {
                    ModelId = options.Current.Model,
                    FunctionChoiceBehavior = tools,
                    ExtensionData = new Dictionary<string, object>
                    {
                        ["temperature"] = options.Temperature,
                        ["top_p"] = options.TopP,
                        ["max_tokens"] = options.MaxTokens,
                    },
                };
        }
    }
}
