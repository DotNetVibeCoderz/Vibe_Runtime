using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Threading;
using Microsoft.Web.WebView2.Core;
using Microsoft.Win32;
using RustNet.Designer.Assistant;

namespace RustNet.Designer;

/// <summary>
/// Jack The Code Bender's panel: sessions on the left of its own drawer, the
/// transcript in a WebView2, and a composer that can carry uploads.
///
/// The transcript is served from a virtual host mapped to the assistant's data
/// directory rather than pushed in with NavigateToString, so the page has a real
/// https origin — attached images can load and the clipboard API works.
/// </summary>
public partial class ChatPanel : UserControl
{
    private readonly ObservableCollection<ChatSession> _sessions = new();
    private readonly List<ChatAttachment> _pending = new();

    private AssistantOptions _options = null!;
    private SessionStore _store = null!;
    private AttachmentStore _attachments = null!;
    private AssistantService _service = null!;
    private IDesignerBridge _designer = null!;

    private ChatSession? _current;
    private CancellationTokenSource? _turn;
    private bool _ready;
    private bool _suppressPickers;
    private bool _webViewUsable;

    // Streaming is flushed on a timer: re-rendering markdown per token would
    // spend more time in Markdig than in the model.
    private readonly DispatcherTimer _flush = new() { Interval = TimeSpan.FromMilliseconds(90) };
    private readonly System.Text.StringBuilder _streamed = new();
    private readonly List<string> _turnTools = new();
    private bool _dirty;

    public ChatPanel()
    {
        InitializeComponent();
        _flush.Tick += (_, _) => FlushStream();
    }

    /// <summary>Raised when the person closes the panel from its own header.</summary>
    public event EventHandler? HideRequested;

    /// <summary>Status text for the window's readout strip.</summary>
    public event EventHandler<string>? Status;

    // ---- lifetime ------------------------------------------------------

    /// <summary>
    /// Wire the panel to the Designer. Safe to call once; the WebView2 warms up
    /// in the background and the panel stays usable if it never arrives.
    /// </summary>
    public async void Initialize(IDesignerBridge designer)
    {
        _designer = designer;
        _options = AssistantOptions.Load();
        _store = new SessionStore(_options.DataDirectory);
        _attachments = new AttachmentStore(_store, _options);
        _service = new AssistantService(_options, _store, _attachments, designer);

        LoadPickers();
        LoadSettingsFields();
        BuildPromptGallery();

        SessionList.ItemsSource = _sessions;
        foreach (ChatSession s in _store.LoadAll())
        {
            _sessions.Add(s);
        }
        if (_sessions.Count == 0)
        {
            _sessions.Add(NewSession());
        }

        await InitializeWebViewAsync();
        _ready = true;
        SelectSession(_sessions[0]);
    }

    private async Task InitializeWebViewAsync()
    {
        try
        {
            string userData = Path.Combine(_options.DataDirectory, "webview");
            Directory.CreateDirectory(userData);
            CoreWebView2Environment env = await CoreWebView2Environment.CreateAsync(null, userData);
            await Transcript.EnsureCoreWebView2Async(env);

            CoreWebView2 core = Transcript.CoreWebView2;
            core.SetVirtualHostNameToFolderMapping(
                AttachmentStore.VirtualHost, _options.DataDirectory, CoreWebView2HostResourceAccessKind.Allow);
            core.Settings.AreDefaultContextMenusEnabled = true;
            core.Settings.IsStatusBarEnabled = false;
            core.Settings.AreBrowserAcceleratorKeysEnabled = false;
            core.WebMessageReceived += OnWebMessage;
            // Nothing in the transcript should navigate the panel itself.
            core.NewWindowRequested += (_, e) =>
            {
                e.Handled = true;
                OpenExternally(e.Uri);
            };
            _webViewUsable = true;
        }
        catch (Exception ex)
        {
            // No WebView2 runtime, or it could not start. Fall back to plain
            // text so the assistant still answers.
            _webViewUsable = false;
            Transcript.Visibility = Visibility.Collapsed;
            FallbackScroll.Visibility = Visibility.Visible;
            Report("Rich transcript unavailable (" + ex.Message.Split('\n')[0] + "); showing plain text.");
        }
    }

    // ---- sessions ------------------------------------------------------

    private ChatSession NewSession()
    {
        var session = new ChatSession
        {
            Provider = _options.Provider.ToString(),
            Model = _options.Current.Model,
        };
        _store.Save(session);
        return session;
    }

    private void SelectSession(ChatSession session)
    {
        _current = session;
        SessionList.SelectedItem = session;
        ClearPending();
        RenderTranscript();
    }

    private void OnSessionSelected(object sender, SelectionChangedEventArgs e)
    {
        if (!_ready || SessionList.SelectedItem is not ChatSession s || ReferenceEquals(s, _current))
        {
            return;
        }
        SelectSession(s);
        SessionsToggle.IsChecked = false;
    }

    private void OnNewSession(object sender, RoutedEventArgs e)
    {
        ChatSession session = NewSession();
        _sessions.Insert(0, session);
        SelectSession(session);
        SessionsToggle.IsChecked = false;
        Composer.Focus();
    }

    private void OnResetSession(object sender, RoutedEventArgs e)
    {
        if (_current == null)
        {
            return;
        }
        if (MessageBox.Show(
                $"Empty \"{_current.Title}\"? Its messages and uploads are deleted; the session stays in the list.",
                "Reset session", MessageBoxButton.OKCancel, MessageBoxImage.Question) != MessageBoxResult.OK)
        {
            return;
        }
        _store.Reset(_current);
        RefreshSessionRow(_current);
        RenderTranscript();
        Report("Session reset.");
    }

    private void OnDeleteSession(object sender, RoutedEventArgs e)
    {
        if (_current == null)
        {
            return;
        }
        if (MessageBox.Show(
                $"Delete \"{_current.Title}\" and its uploads? This cannot be undone.",
                "Delete session", MessageBoxButton.OKCancel, MessageBoxImage.Warning) != MessageBoxResult.OK)
        {
            return;
        }
        ChatSession gone = _current;
        _store.Delete(gone);
        _sessions.Remove(gone);
        if (_sessions.Count == 0)
        {
            _sessions.Add(NewSession());
        }
        SelectSession(_sessions[0]);
        Report("Session deleted.");
    }

    /// <summary>Re-seat a session in the list so its message count and title refresh.</summary>
    private void RefreshSessionRow(ChatSession session)
    {
        int i = _sessions.IndexOf(session);
        if (i >= 0)
        {
            _sessions.RemoveAt(i);
            _sessions.Insert(i, session);
            SessionList.SelectedItem = session;
        }
    }

    // ---- transcript ----------------------------------------------------

    private void RenderTranscript()
    {
        if (_current == null)
        {
            return;
        }
        string html = MarkdownRenderer.Document(_current.Messages, EmptyState());

        if (!_webViewUsable)
        {
            var sb = new System.Text.StringBuilder();
            foreach (ChatMessage m in _current.Messages)
            {
                sb.AppendLine(m.Role == ChatRole.User ? "YOU" : "JACK");
                sb.AppendLine(m.Text).AppendLine();
            }
            FallbackText.Text = sb.Length > 0 ? sb.ToString() : "Ask Jack something.";
            return;
        }

        try
        {
            string path = Path.Combine(_options.DataDirectory, "transcript.html");
            File.WriteAllText(path, html);
            Transcript.CoreWebView2.Navigate($"https://{AttachmentStore.VirtualHost}/transcript.html?t={Environment.TickCount}");
        }
        catch (Exception ex)
        {
            Report("Could not draw the transcript: " + ex.Message);
        }
    }

    private string EmptyState()
    {
        string provider = _options.Provider.ToString();
        string model = _options.Current.Model;
        bool configured = _options.IsProviderConfigured(_options.Provider);
        string keyLine = configured
            ? $"Wired to <code>{provider}</code> / <code>{model}</code>."
            : $"<code>{provider}</code> has no API key yet. Put one in <code>Assistant.{provider}.ApiKey</code> "
              + "in app.config, or export the environment variable the placeholder names.";

        return $"""
            <h2>Jack The Code Bender</h2>
            <p>{keyLine}</p>
            <p>Ask for a screen and it lands on your canvas; ask for app code and it lands in the code pane.</p>
            <ul>
              <li>“Design a 320x240 boiler dashboard with flow, return and burner load, then apply it.”</li>
              <li>“Critique the layout on my canvas and apply a better version.”</li>
              <li>“Write the MQTT loop for this screen.”</li>
            </ul>
            <p>Press <strong>Prompts</strong> for a gallery of these.</p>
            """;
    }

    private void Append(ChatMessage message, bool streaming = false)
    {
        if (!_webViewUsable)
        {
            RenderTranscript();
            return;
        }
        string html = MarkdownRenderer.RenderMessage(message, streaming);
        Exec($"rn.append({MarkdownRenderer.Js(html)})");
    }

    private void Exec(string script)
    {
        if (_webViewUsable && Transcript.CoreWebView2 != null)
        {
            _ = Transcript.CoreWebView2.ExecuteScriptAsync(script);
        }
    }

    private void OnWebMessage(object? sender, CoreWebView2WebMessageReceivedEventArgs e)
    {
        string action, code, lang;
        try
        {
            using JsonDocument doc = JsonDocument.Parse(e.WebMessageAsJson);
            action = Text(doc, "action");
            code = Text(doc, "code");
            lang = Text(doc, "lang");
        }
        catch (JsonException)
        {
            return;
        }

        switch (action)
        {
            case "apply-xml":
                try
                {
                    _designer.ApplyLayoutXml(code);
                    Report("Layout applied from the transcript.");
                }
                catch (Exception ex)
                {
                    MessageBox.Show("That layout did not parse:\n\n" + ex.Message, "Apply to canvas");
                }
                break;

            case "to-code":
                _designer.SetGeneratedCode(FileNameFor(lang), lang, code);
                Report("Sent to the code pane.");
                break;

            case "open-url":
                OpenExternally(code);
                break;
        }

        static string Text(JsonDocument d, string name)
            => d.RootElement.TryGetProperty(name, out JsonElement v) && v.ValueKind == JsonValueKind.String
                ? v.GetString() ?? "" : "";
    }

    private static string FileNameFor(string language) => CodeHighlighter.Normalize(language) switch
    {
        "csharp" => "Program.cs",
        "xml" => "ui.xml",
        "json" => "data.json",
        "rust" => "main.rs",
        "bash" => "commands.sh",
        _ => "snippet.txt",
    };

    private void OpenExternally(string url)
    {
        try
        {
            Process.Start(new ProcessStartInfo(url) { UseShellExecute = true });
        }
        catch (Exception ex)
        {
            Report("Could not open " + url + ": " + ex.Message);
        }
    }

    // ---- the turn ------------------------------------------------------

    private async void OnSend(object sender, RoutedEventArgs e) => await SendAsync();

    private void OnStop(object sender, RoutedEventArgs e)
    {
        _turn?.Cancel();
        Report("Stopped.");
    }

    private void OnComposerKeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key == Key.Enter && (Keyboard.Modifiers & ModifierKeys.Shift) == 0)
        {
            e.Handled = true;
            _ = SendAsync();
        }
    }

    private async Task SendAsync()
    {
        if (_current == null || _turn != null)
        {
            return;
        }
        string text = Composer.Text.Trim();
        if (text.Length == 0 && _pending.Count == 0)
        {
            return;
        }

        var userMessage = new ChatMessage { Role = ChatRole.User, Text = text };
        userMessage.Attachments.AddRange(_pending);
        _current.Messages.Add(userMessage);
        if (_current.Messages.Count == 1)
        {
            _current.RetitleFromFirstMessage();
        }
        _current.Provider = _options.Provider.ToString();
        _current.Model = _options.Current.Model;
        _store.Save(_current);

        Composer.Clear();
        ClearPending();
        Append(userMessage);
        RefreshSessionRow(_current);

        var live = new ChatMessage
        {
            Role = ChatRole.Assistant,
            Model = $"{_options.Provider}/{_options.Current.Model}",
        };
        _streamed.Clear();
        _turnTools.Clear();
        Append(live, streaming: true);
        SetBusy(true);

        _turn = new CancellationTokenSource();
        try
        {
            ChatMessage reply = await _service.SendAsync(
                _current, userMessage,
                onDelta: piece => Dispatcher.Invoke(() => { _streamed.Append(piece); _dirty = true; }),
                onTool: name => Dispatcher.Invoke(() => AddToolChip(name)),
                _turn.Token);

            reply.ToolCalls.AddRange(_turnTools);
            _current.Messages.Add(reply);
            _store.Save(_current);
            Exec($"rn.setLive({MarkdownRenderer.Js(MarkdownRenderer.RenderMessage(reply))})");
            Report(reply.ToolCalls.Count > 0
                ? $"Answered using {reply.ToolCalls.Count} function call(s)."
                : "Answered.");
        }
        catch (OperationCanceledException)
        {
            FinishPartial(live, "_(stopped)_");
        }
        catch (Exception ex)
        {
            FinishPartial(live, "**That turn failed.**\n\n" + ex.Message, isError: true);
            Report("Turn failed: " + ex.Message);
        }
        finally
        {
            SetBusy(false);
            _turn?.Dispose();
            _turn = null;
            if (_current != null)
            {
                RefreshSessionRow(_current);
            }
        }
    }

    /// <summary>
    /// Keep whatever streamed before the failure — a half answer with the error
    /// under it is more use than an empty bubble.
    /// </summary>
    private void FinishPartial(ChatMessage live, string note, bool isError = false)
    {
        FlushStream();
        live.Text = _streamed.Length > 0 ? _streamed + "\n\n" + note : note;
        live.IsError = isError;
        live.ToolCalls.AddRange(_turnTools);
        if (_current != null)
        {
            _current.Messages.Add(live);
            _store.Save(_current);
        }
        Exec($"rn.setLive({MarkdownRenderer.Js(MarkdownRenderer.RenderMessage(live))})");
    }

    private void AddToolChip(string name)
    {
        // The same call can be reported by both the SK filter and the
        // IChatClient tap; only the first sighting in a row is shown.
        if (_turnTools.Count > 0 && _turnTools[^1] == name)
        {
            return;
        }
        _turnTools.Add(name);
        Exec($"rn.setTools({MarkdownRenderer.Js(MarkdownRenderer.RenderTools(_turnTools))})");
        Report("Calling " + name + "…");
    }

    private void FlushStream()
    {
        if (!_dirty)
        {
            return;
        }
        _dirty = false;
        Exec($"rn.setBody({MarkdownRenderer.Js(MarkdownRenderer.RenderBody(_streamed.ToString()))})");
    }

    private void SetBusy(bool busy)
    {
        SendButton.Visibility = busy ? Visibility.Collapsed : Visibility.Visible;
        StopButton.Visibility = busy ? Visibility.Visible : Visibility.Collapsed;
        Composer.IsEnabled = !busy;
        SignalMark.Background = busy
            ? (System.Windows.Media.Brush)FindResource("Cyan")
            : (System.Windows.Media.Brush)FindResource("CyanDim");
        if (busy)
        {
            _flush.Start();
        }
        else
        {
            _flush.Stop();
            FlushStream();
            Exec("rn.settle()");
        }
    }

    // ---- attachments ---------------------------------------------------

    private void OnAttachImage(object sender, RoutedEventArgs e) => Attach(
        "Images|*.png;*.jpg;*.jpeg;*.gif;*.webp;*.bmp|All files|*.*");

    private void OnAttachDocument(object sender, RoutedEventArgs e) => Attach(
        "Documents|*.md;*.txt;*.cs;*.xml;*.json;*.csv;*.log;*.yaml;*.toml;*.rs;*.pdf|All files|*.*");

    private void Attach(string filter)
    {
        if (_current == null)
        {
            return;
        }
        var dlg = new OpenFileDialog { Filter = filter, Multiselect = true };
        if (dlg.ShowDialog() != true)
        {
            return;
        }
        foreach (string file in dlg.FileNames)
        {
            try
            {
                _pending.Add(_attachments.Add(_current, file));
            }
            catch (Exception ex)
            {
                MessageBox.Show(ex.Message, "Attach");
            }
        }
        RenderPending();
    }

    private void RenderPending()
    {
        AttachmentStrip.Items.Clear();
        foreach (ChatAttachment a in _pending)
        {
            var chip = new Border
            {
                Background = (System.Windows.Media.Brush)FindResource("Sunken"),
                BorderBrush = (System.Windows.Media.Brush)FindResource(
                    a.Kind == AttachmentKind.Image ? "AmberDim" : "Rail"),
                BorderThickness = new Thickness(1),
                CornerRadius = new CornerRadius(2),
                Padding = new Thickness(6, 2, 3, 2),
                Margin = new Thickness(0, 0, 4, 4),
            };
            var row = new StackPanel { Orientation = Orientation.Horizontal };
            row.Children.Add(new TextBlock
            {
                Text = $"{a.FileName}  {a.SizeBytes / 1024} KB",
                FontFamily = (System.Windows.Media.FontFamily)FindResource("FontMono"),
                FontSize = 10,
                Foreground = (System.Windows.Media.Brush)FindResource("Muted"),
                VerticalAlignment = VerticalAlignment.Center,
            });
            var remove = new Button
            {
                Style = (Style)FindResource("GhostButton"),
                Content = "✕",
                FontSize = 9,
                Padding = new Thickness(4, 0, 4, 0),
                Margin = new Thickness(4, 0, 0, 0),
                Tag = a,
                ToolTip = "Remove this upload from the message",
            };
            remove.Click += (s, _) =>
            {
                if (((Button)s).Tag is ChatAttachment target)
                {
                    _pending.Remove(target);
                    TryDelete(target.StoredPath);
                    RenderPending();
                }
            };
            row.Children.Add(remove);
            chip.Child = row;
            AttachmentStrip.Items.Add(chip);
        }
    }

    private void ClearPending()
    {
        _pending.Clear();
        AttachmentStrip.Items.Clear();
    }

    private static void TryDelete(string path)
    {
        try
        {
            if (File.Exists(path))
            {
                File.Delete(path);
            }
        }
        catch (IOException)
        {
            // An upload left behind is harmless; it goes with the session.
        }
    }

    // ---- pickers and settings ------------------------------------------

    private void LoadPickers()
    {
        _suppressPickers = true;
        ProviderBox.Items.Clear();
        foreach (AiProvider p in Enum.GetValues<AiProvider>())
        {
            ProviderBox.Items.Add(p.ToString());
        }
        ProviderBox.SelectedItem = _options.Provider.ToString();
        LoadModels();
        ToolsToggle.IsChecked = _options.ToolsEnabled;
        _suppressPickers = false;
    }

    private void LoadModels()
    {
        ModelBox.Items.Clear();
        foreach (string m in _options.Current.Models)
        {
            ModelBox.Items.Add(m);
        }
        if (_options.Current.Model.Length > 0)
        {
            ModelBox.SelectedItem = _options.Current.Model;
        }
    }

    private void OnProviderChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_suppressPickers || ProviderBox.SelectedItem is not string name)
        {
            return;
        }
        _options.Provider = AssistantOptions.ParseProvider(name, _options.Provider);
        _suppressPickers = true;
        LoadModels();
        _suppressPickers = false;
        _service?.Invalidate();
        _options.Save();
        LoadSettingsFields();
        Report($"Provider is {_options.Provider}.");
    }

    private void OnModelChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_suppressPickers || ModelBox.SelectedItem is not string model)
        {
            return;
        }
        _options.Current.Model = model;
        _service?.Invalidate();
        _options.Save();
        Report("Model is " + model + ".");
    }

    private void OnToolsToggled(object sender, RoutedEventArgs e)
    {
        if (_suppressPickers || _options == null)
        {
            return;
        }
        _options.ToolsEnabled = ToolsToggle.IsChecked == true;
        _service?.Invalidate();
        _options.Save();
        LoadSettingsFields();
        Report(_options.ToolsEnabled ? "Functions enabled." : "Functions disabled.");
    }

    private void LoadSettingsFields()
    {
        PersonaBox.Text = _options.Persona;
        TemperatureBox.Text = _options.Temperature.ToString("0.##", CultureInfo.InvariantCulture);
        TopPBox.Text = _options.TopP.ToString("0.##", CultureInfo.InvariantCulture);
        MaxTokensBox.Text = _options.MaxTokens.ToString(CultureInfo.InvariantCulture);
        HistoryBox.Text = _options.HistoryTurns.ToString(CultureInfo.InvariantCulture);
        StreamingCheck.IsChecked = _options.Streaming;
        ToolListText.Text = _options.ToolsEnabled
            ? string.Join("  ", _service?.ToolNames ?? Array.Empty<string>())
            : "(functions are off)";
        KeyStatusText.Text = string.Join("\n", new[]
        {
            Line(AiProvider.OpenAI), Line(AiProvider.Anthropic), Line(AiProvider.Gemini), Line(AiProvider.Ollama),
            "tavily      " + (_options.TavilyApiKey.Length > 0 ? "set" : "not set — search is unavailable"),
        });

        string Line(AiProvider p) =>
            $"{_options.For(p).Name,-11} {(_options.IsProviderConfigured(p) ? "ready" : "not configured")}";
    }

    private void OnSaveSettings(object sender, RoutedEventArgs e)
    {
        _options.Persona = PersonaBox.Text;
        if (double.TryParse(TemperatureBox.Text, NumberStyles.Float, CultureInfo.InvariantCulture, out double t))
        {
            _options.Temperature = Math.Clamp(t, 0, 2);
        }
        if (double.TryParse(TopPBox.Text, NumberStyles.Float, CultureInfo.InvariantCulture, out double p))
        {
            _options.TopP = Math.Clamp(p, 0, 1);
        }
        if (int.TryParse(MaxTokensBox.Text, out int max))
        {
            _options.MaxTokens = Math.Clamp(max, 256, 200_000);
        }
        if (int.TryParse(HistoryBox.Text, out int turns))
        {
            _options.HistoryTurns = Math.Max(0, turns);
        }
        _options.Streaming = StreamingCheck.IsChecked == true;
        _options.Save();
        _service.Invalidate();
        LoadSettingsFields();
        Report("Settings saved to app.config.");
    }

    private void OnResetPersona(object sender, RoutedEventArgs e)
    {
        PersonaBox.Text = PromptLibrary.DefaultPersona;
    }

    // ---- drawers -------------------------------------------------------

    private bool _promptsOpen;

    /// <summary>
    /// Show at most one drawer, and take the transcript out of the way while one
    /// is open. WebView2 is a child HWND: WPF content cannot be drawn on top of
    /// it, so a drawer laid over it would simply not appear.
    /// </summary>
    private void SyncDrawers()
    {
        Border? open =
            _promptsOpen ? PromptDrawer
            : SettingsToggle.IsChecked == true ? SettingsDrawer
            : SessionsToggle.IsChecked == true ? SessionsDrawer
            : null;

        SessionsDrawer.Visibility = Vis(SessionsDrawer);
        SettingsDrawer.Visibility = Vis(SettingsDrawer);
        PromptDrawer.Visibility = Vis(PromptDrawer);

        Transcript.Visibility = open == null && _webViewUsable ? Visibility.Visible : Visibility.Collapsed;
        FallbackScroll.Visibility = open == null && !_webViewUsable ? Visibility.Visible : Visibility.Collapsed;

        Visibility Vis(Border drawer)
            => ReferenceEquals(drawer, open) ? Visibility.Visible : Visibility.Collapsed;
    }

    private void OnSessionsToggled(object sender, RoutedEventArgs e)
    {
        if (SessionsToggle.IsChecked == true)
        {
            SettingsToggle.IsChecked = false;
            _promptsOpen = false;
        }
        SyncDrawers();
    }

    private void OnSettingsToggled(object sender, RoutedEventArgs e)
    {
        if (SettingsToggle.IsChecked == true)
        {
            SessionsToggle.IsChecked = false;
            _promptsOpen = false;
            LoadSettingsFields();
        }
        SyncDrawers();
    }

    private void OnShowPrompts(object sender, RoutedEventArgs e)
    {
        _promptsOpen = true;
        SessionsToggle.IsChecked = false;
        SettingsToggle.IsChecked = false;
        SyncDrawers();
    }

    private void OnClosePrompts(object sender, RoutedEventArgs e)
    {
        _promptsOpen = false;
        SyncDrawers();
    }

    private void OnHide(object sender, RoutedEventArgs e) => HideRequested?.Invoke(this, EventArgs.Empty);

    private void BuildPromptGallery()
    {
        string? lastCategory = null;
        foreach (PromptTemplate template in PromptLibrary.All)
        {
            if (template.Category != lastCategory)
            {
                lastCategory = template.Category;
                PromptList.Children.Add(new TextBlock
                {
                    Text = template.Category.ToUpperInvariant(),
                    Style = (Style)FindResource("PaneHeading"),
                    Margin = new Thickness(10, 12, 10, 4),
                });
            }
            var button = new Button
            {
                Style = (Style)FindResource("GhostButton"),
                HorizontalContentAlignment = HorizontalAlignment.Left,
                Margin = new Thickness(6, 0, 6, 1),
                Padding = new Thickness(6, 4, 6, 4),
                Foreground = (System.Windows.Media.Brush)FindResource("Ink"),
                Content = new TextBlock { Text = template.Title, TextWrapping = TextWrapping.Wrap },
                ToolTip = new TextBlock { Text = template.Text, TextWrapping = TextWrapping.Wrap, MaxWidth = 420 },
                Tag = template,
            };
            button.Click += (s, _) =>
            {
                if (((Button)s).Tag is PromptTemplate chosen)
                {
                    Composer.Text = chosen.Text;
                    Composer.CaretIndex = Composer.Text.Length;
                    _promptsOpen = false;
                    SyncDrawers();
                    Composer.Focus();
                }
            };
            PromptList.Children.Add(button);
        }
    }

    // ---- misc ----------------------------------------------------------

    /// <summary>Put the caret in the composer — what Ctrl+J should do.</summary>
    public void FocusComposer() => Composer.Focus();

    /// <summary>
    /// Drop text into the composer without sending it, so the person can edit
    /// the request first. Used by "Send to assistant" in the code pane.
    /// </summary>
    public void Compose(string text)
    {
        Composer.Text = text;
        Composer.CaretIndex = Composer.Text.Length;
        _promptsOpen = false;
        SessionsToggle.IsChecked = false;
        SettingsToggle.IsChecked = false;
        SyncDrawers();
        Composer.Focus();
    }

    private void Report(string message) => Status?.Invoke(this, message);
}
