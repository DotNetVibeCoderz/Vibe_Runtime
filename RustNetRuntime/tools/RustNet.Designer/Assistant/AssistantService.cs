using System;
using System.Collections.Generic;
using System.IO;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.SemanticKernel;
using Microsoft.SemanticKernel.ChatCompletion;
using RustNet.Designer.Assistant.Plugins;

namespace RustNet.Designer.Assistant;

/// <summary>
/// One turn of conversation, end to end: assemble the prompt from the session,
/// hand it to the kernel with the tools registered, stream the reply back, and
/// report which functions the model called along the way.
///
/// The kernel is rebuilt whenever the provider or model changes, so switching
/// model mid-session is just a picker change.
/// </summary>
public sealed class AssistantService : IDisposable
{
    private readonly AssistantOptions _options;
    private readonly AttachmentStore _attachments;
    private readonly IDesignerBridge _designer;
    private readonly WebPlugin _web;
    private readonly ToolTap _tap = new();

    private Kernel? _kernel;
    private string _kernelKey = "";
    private ChatSession? _session;

    public AssistantService(
        AssistantOptions options,
        SessionStore sessions,
        AttachmentStore attachments,
        IDesignerBridge designer)
    {
        _options = options;
        _attachments = attachments;
        _designer = designer;
        _web = new WebPlugin(options);
        Sessions = sessions;
    }

    public SessionStore Sessions { get; }
    public AssistantOptions Options => _options;

    /// <summary>Names of the kernel functions the assistant can call, for the UI.</summary>
    public IReadOnlyList<string> ToolNames
    {
        get
        {
            var names = new List<string>();
            foreach (KernelPlugin plugin in BuildPlugins())
            {
                foreach (KernelFunction f in plugin)
                {
                    names.Add(f.Name);
                }
            }
            names.Sort(StringComparer.Ordinal);
            return names;
        }
    }

    /// <summary>Drop the cached kernel; the next turn rebuilds it from settings.</summary>
    public void Invalidate() => _kernelKey = "";

    // ---- the turn ------------------------------------------------------

    /// <summary>
    /// Send <paramref name="userMessage"/> (already appended to
    /// <paramref name="session"/>) and stream the reply. <paramref name="onDelta"/>
    /// receives text as it arrives; <paramref name="onTool"/> receives a
    /// function name each time the model calls one. Returns the assistant
    /// message, which the caller appends and saves.
    /// </summary>
    public async Task<ChatMessage> SendAsync(
        ChatSession session,
        ChatMessage userMessage,
        Action<string> onDelta,
        Action<string> onTool,
        CancellationToken cancellationToken)
    {
        _session = session;
        Kernel kernel = GetKernel(onTool);
        IChatCompletionService chat = kernel.GetRequiredService<IChatCompletionService>();
        PromptExecutionSettings settings = KernelFactory.CreateSettings(_options);

        ChatHistory history = BuildHistory(session, userMessage);

        var reply = new ChatMessage
        {
            Role = ChatRole.Assistant,
            Model = $"{_options.Provider}/{_options.Current.Model}",
        };
        var text = new StringBuilder();

        if (_options.Streaming)
        {
            await foreach (StreamingChatMessageContent chunk in
                chat.GetStreamingChatMessageContentsAsync(history, settings, kernel, cancellationToken)
                    .ConfigureAwait(false))
            {
                if (chunk.Content is { Length: > 0 } piece)
                {
                    text.Append(piece);
                    onDelta(piece);
                }
            }
        }
        else
        {
            IReadOnlyList<Microsoft.SemanticKernel.ChatMessageContent> results =
                await chat.GetChatMessageContentsAsync(history, settings, kernel, cancellationToken)
                    .ConfigureAwait(false);
            foreach (Microsoft.SemanticKernel.ChatMessageContent content in results)
            {
                if (content.Content is { Length: > 0 } piece)
                {
                    text.Append(piece);
                    onDelta(piece);
                }
            }
        }

        reply.Text = text.ToString().Trim();
        if (reply.Text.Length == 0)
        {
            // A turn that only ran tools still needs to say something, or the
            // transcript shows an empty bubble.
            reply.Text = "_(no text in the reply — the tools above are the result)_";
        }
        return reply;
    }

    // ---- prompt assembly -----------------------------------------------

    private ChatHistory BuildHistory(ChatSession session, ChatMessage pending)
    {
        var history = new ChatHistory();
        history.AddSystemMessage(_options.Persona + "\n\n" + LiveContext());

        // Replay the recent transcript. HistoryTurns of 0 means all of it.
        List<ChatMessage> past = session.Messages;
        int start = _options.HistoryTurns > 0 && past.Count > _options.HistoryTurns
            ? past.Count - _options.HistoryTurns
            : 0;
        for (int i = start; i < past.Count; i++)
        {
            ChatMessage m = past[i];
            if (ReferenceEquals(m, pending) || m.Text.Length == 0)
            {
                continue;
            }
            if (m.Role == ChatRole.User)
            {
                history.Add(UserTurn(m));
            }
            else if (m.Role == ChatRole.Assistant && !m.IsError)
            {
                history.AddAssistantMessage(m.Text);
            }
        }
        history.Add(UserTurn(pending));
        return history;
    }

    /// <summary>
    /// A user turn is text plus image content for every attached image. Images
    /// go as bytes, not as their file:// URL — a hosted model cannot open a path
    /// on this machine.
    /// </summary>
    private Microsoft.SemanticKernel.ChatMessageContent UserTurn(ChatMessage m)
    {
        var items = new ChatMessageContentItemCollection();
        var text = new StringBuilder(m.Text);

        foreach (ChatAttachment a in m.Attachments)
        {
            if (a.Kind == AttachmentKind.Image)
            {
                try
                {
                    items.Add(new ImageContent(AttachmentStore.ReadBytes(a), a.MimeType));
                }
                catch (IOException ex)
                {
                    text.Append($"\n\n[attached image {a.FileName} could not be read: {ex.Message}]");
                }
                continue;
            }

            text.Append($"\n\n---\nAttached document: [{a.FileName}]({a.Url})");
            if (a.TextExcerpt.Length > 0)
            {
                text.Append($"\n\n```\n{a.TextExcerpt}\n```");
                if (a.Truncated)
                {
                    text.Append($"\n[excerpt only — call read_attachment(\"{a.FileName}\") for the rest]");
                }
            }
            else
            {
                text.Append($"\n[binary or unsupported type ({a.MimeType}); its contents are not inlined]");
            }
        }

        items.Insert(0, new TextContent(text.ToString()));
        return new Microsoft.SemanticKernel.ChatMessageContent(AuthorRole.User, items);
    }

    /// <summary>
    /// What is true right now, appended to the persona: the panel being
    /// designed, what is on it, and where the checkout is. Without this the
    /// model has to call functions just to learn the basics.
    /// </summary>
    private string LiveContext()
    {
        (int w, int h) = _designer.GetPanelSize();
        string layout = _designer.GetLayoutXml();
        string trimmedLayout = layout.Length <= 2000 ? layout : layout.Substring(0, 2000) + "\n<!-- truncated -->";
        return $"""
            # Right now

            - Date: {DateTime.Now:yyyy-MM-dd} ({DateTime.Now.DayOfWeek}).
            - Designer panel: {w}x{h} px. Selected: {_designer.DescribeSelection()}.
            - RustNet checkout: {_options.WorkspaceRoot}
            - Provider: {_options.Provider}, model {_options.Current.Model}.

            The layout currently on the canvas:

            ```xml
            {trimmedLayout}
            ```
            """;
    }

    // ---- kernel --------------------------------------------------------

    private Kernel GetKernel(Action<string> onTool)
    {
        string key = $"{_options.Provider}|{_options.Current.Model}|{_options.Current.Endpoint}|{_options.ToolsEnabled}";
        if (_kernel == null || key != _kernelKey)
        {
            _kernel = KernelFactory.Create(_options, BuildPlugins(), _tap);
            _kernelKey = key;
        }
        // Two reporting paths, because the tool loop lives in different places
        // per provider: the SK filter covers the connectors that run it
        // themselves, the tap covers the IChatClient middleware. Whichever
        // fires, the UI hears about the call once — repeats are collapsed
        // there.
        _tap.OnTool = onTool;
        _kernel.FunctionInvocationFilters.Clear();
        _kernel.FunctionInvocationFilters.Add(new ToolReporter(onTool));
        return _kernel;
    }

    private List<KernelPlugin> BuildPlugins()
    {
        var plugins = new List<KernelPlugin>
        {
            KernelPluginFactory.CreateFromObject(new DesignPlugin(_designer), "design"),
            KernelPluginFactory.CreateFromObject(new TimePlugin(), "time"),
            KernelPluginFactory.CreateFromObject(new MathPlugin(), "math"),
            KernelPluginFactory.CreateFromObject(
                new WorkspacePlugin(_options, _attachments, () => _session), "workspace"),
        };
        if (_options.ToolsEnabled)
        {
            plugins.Add(KernelPluginFactory.CreateFromObject(_web, "web"));
        }
        return plugins;
    }

    /// <summary>Reports every kernel function the model invokes to the UI.</summary>
    private sealed class ToolReporter : IFunctionInvocationFilter
    {
        private readonly Action<string> _onTool;

        public ToolReporter(Action<string> onTool) => _onTool = onTool;

        public async Task OnFunctionInvocationAsync(
            FunctionInvocationContext context, Func<FunctionInvocationContext, Task> next)
        {
            _onTool(context.Function.Name);
            await next(context).ConfigureAwait(false);
        }
    }

    public void Dispose() => _web.Dispose();
}
