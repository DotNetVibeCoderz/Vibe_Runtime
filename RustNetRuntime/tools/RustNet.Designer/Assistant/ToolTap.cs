using System;
using System.Collections.Generic;
using System.Runtime.CompilerServices;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Extensions.AI;

// This namespace has its own ChatMessage (the transcript's), which would win
// over the using directive — the middleware needs the M.E.AI one.
using AIChatMessage = Microsoft.Extensions.AI.ChatMessage;

namespace RustNet.Designer.Assistant;

/// <summary>
/// Where "the model called a function" notifications land. One instance lives
/// for the life of the service and its callback is swapped each turn, so the
/// kernel can be cached across turns while the UI target changes.
/// </summary>
public sealed class ToolTap
{
    public Action<string>? OnTool { get; set; }

    public void Report(string functionName) => OnTool?.Invoke(functionName);
}

/// <summary>
/// Reports tool calls on the <see cref="IChatClient"/> path. The SK connectors
/// raise function-invocation filters for their own tool loops, but the Anthropic
/// bridge runs its loop in <c>UseFunctionInvocation()</c> middleware instead —
/// so this sits in front of that pipeline and reads the function calls straight
/// off the model's response.
/// </summary>
internal sealed class ToolTapChatClient : DelegatingChatClient
{
    private readonly ToolTap _tap;

    public ToolTapChatClient(IChatClient inner, ToolTap tap) : base(inner) => _tap = tap;

    public override async Task<ChatResponse> GetResponseAsync(
        IEnumerable<AIChatMessage> messages,
        ChatOptions? options = null,
        CancellationToken cancellationToken = default)
    {
        ChatResponse response = await base.GetResponseAsync(messages, options, cancellationToken)
            .ConfigureAwait(false);
        foreach (AIChatMessage message in response.Messages)
        {
            foreach (AIContent content in message.Contents)
            {
                if (content is FunctionCallContent call)
                {
                    _tap.Report(call.Name);
                }
            }
        }
        return response;
    }

    public override async IAsyncEnumerable<ChatResponseUpdate> GetStreamingResponseAsync(
        IEnumerable<AIChatMessage> messages,
        ChatOptions? options = null,
        [EnumeratorCancellation] CancellationToken cancellationToken = default)
    {
        await foreach (ChatResponseUpdate update in
            base.GetStreamingResponseAsync(messages, options, cancellationToken).ConfigureAwait(false))
        {
            foreach (AIContent content in update.Contents)
            {
                if (content is FunctionCallContent call)
                {
                    _tap.Report(call.Name);
                }
            }
            yield return update;
        }
    }
}
