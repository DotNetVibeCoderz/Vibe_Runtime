using System;
using System.Collections.Generic;
using System.Net;
using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;
using Markdig;

namespace RustNet.Designer.Assistant;

/// <summary>
/// Renders the transcript for the WebView2 host: Markdig to HTML, then the
/// fenced code blocks rewritten into a card with a language chip and actions
/// that post back to the Designer (apply an XML layout to the canvas, send C#
/// to the code pane, copy).
///
/// The page is styled as an instrument readout rather than a chat app: mono
/// type for anything the device would show as data, one warm accent for the
/// hardware side and one cool accent for the assistant's own voice.
/// </summary>
public static class MarkdownRenderer
{
    private static readonly MarkdownPipeline Pipeline = new MarkdownPipelineBuilder()
        .UseAdvancedExtensions()   // tables, task lists, autolinks, footnotes, emphasis extras
        .UseMediaLinks()           // image links to video/audio become <video>/<audio>
        .UseSoftlineBreakAsHardlineBreak()
        .Build();

    private static readonly Regex CodeBlock = new(
        @"<pre><code(?: class=""language-(?<lang>[^""]*)"")?>(?<body>.*?)</code></pre>",
        RegexOptions.Singleline | RegexOptions.Compiled);

    // ---- fragments -----------------------------------------------------

    /// <summary>The rendered body of one message: markdown to styled HTML.</summary>
    public static string RenderBody(string markdown)
    {
        string html = Markdown.ToHtml(markdown ?? "", Pipeline);
        return CodeBlock.Replace(html, RewriteCodeBlock);
    }

    private static string RewriteCodeBlock(Match m)
    {
        string language = m.Groups["lang"].Success ? m.Groups["lang"].Value : "";
        // Markdig escaped the source; highlight the original text, then escape
        // again per token.
        string code = WebUtility.HtmlDecode(m.Groups["body"].Value).TrimEnd('\n');
        string kind = CodeHighlighter.Normalize(language);
        string label = language.Length > 0 ? language.ToUpperInvariant() : "TEXT";
        // The code travels in a data attribute as base64, not in an inline
        // handler: source full of quotes and apostrophes would break out of any
        // attribute quoting we could pick.
        string payload = Convert.ToBase64String(Encoding.UTF8.GetBytes(code));

        var actions = new StringBuilder();
        if (kind == "xml" && code.TrimStart().StartsWith("<window", StringComparison.OrdinalIgnoreCase))
        {
            // Only offer the canvas action for something that could be a layout.
            actions.Append("<button class=\"act primary\" data-act=\"apply-xml\">Apply to canvas</button>");
        }
        actions.Append("<button class=\"act\" data-act=\"to-code\">Send to code pane</button>");
        actions.Append("<button class=\"act\" data-act=\"copy\">Copy</button>");

        return $"""
            <figure class="code {(kind == "xml" ? "code-xml" : "")}" data-code="{payload}" data-lang="{HtmlAttr(language)}">
              <figcaption><span class="chip">{WebUtility.HtmlEncode(label)}</span><span class="acts">{actions}</span></figcaption>
              <pre><code>{CodeHighlighter.Highlight(code, language)}</code></pre>
            </figure>
            """;
    }

    /// <summary>One turn, eyebrow and all. Streaming replies get an id so the body can be replaced.</summary>
    public static string RenderMessage(ChatMessage message, bool streaming = false)
    {
        string who = message.Role switch
        {
            ChatRole.User => "you",
            ChatRole.Assistant => "jack",
            _ => "system",
        };
        string eyebrow = message.Role switch
        {
            ChatRole.User => "YOU",
            ChatRole.Assistant => "JACK THE CODE BENDER",
            _ => "SYSTEM",
        };

        var sb = new StringBuilder();
        sb.Append($"<article class=\"turn {who}{(message.IsError ? " failed" : "")}\"{(streaming ? " id=\"live\"" : "")}>");
        sb.Append("<header><span class=\"eyebrow\">").Append(eyebrow).Append("</span>");
        if (message.Model.Length > 0)
        {
            sb.Append("<span class=\"byline\">").Append(WebUtility.HtmlEncode(message.Model)).Append("</span>");
        }
        sb.Append($"<time>{message.CreatedUtc.ToLocalTime():HH:mm}</time>");
        if (streaming)
        {
            sb.Append("<span class=\"carrier\" aria-hidden=\"true\"></span>");
        }
        sb.Append("</header>");

        sb.Append("<div class=\"tools\">").Append(RenderTools(message.ToolCalls)).Append("</div>");
        sb.Append("<div class=\"body\">").Append(RenderBody(message.Text)).Append("</div>");

        if (message.Attachments.Count > 0)
        {
            sb.Append("<div class=\"files\">");
            foreach (ChatAttachment a in message.Attachments)
            {
                if (a.Kind == AttachmentKind.Image)
                {
                    // src must be the virtual-host URL: a file:// image is
                    // blocked from the transcript's https origin.
                    string src = a.WebUrl.Length > 0 ? a.WebUrl : a.Url;
                    sb.Append($"<a class=\"thumb\" href=\"{HtmlAttr(a.Url)}\">")
                      .Append($"<img src=\"{HtmlAttr(src)}\" alt=\"{HtmlAttr(a.FileName)}\"></a>");
                }
                else
                {
                    sb.Append($"<a class=\"doc\" href=\"{HtmlAttr(a.Url)}\">")
                      .Append(WebUtility.HtmlEncode(a.FileName))
                      .Append($"<span>{a.SizeBytes / 1024} KB</span></a>");
                }
            }
            sb.Append("</div>");
        }

        sb.Append("</article>");
        return sb.ToString();
    }

    /// <summary>The pills that show which kernel functions ran, in call order.</summary>
    public static string RenderTools(IReadOnlyList<string> toolCalls)
    {
        if (toolCalls.Count == 0)
        {
            return "";
        }
        var sb = new StringBuilder();
        foreach (string name in toolCalls)
        {
            sb.Append("<span class=\"tool\">").Append(WebUtility.HtmlEncode(name)).Append("</span>");
        }
        return sb.ToString();
    }

    // ---- document ------------------------------------------------------

    /// <summary>
    /// The whole page. Called once per session load; afterwards the host pushes
    /// individual turns in through <c>rn.append</c> so the scroll position and
    /// the streaming state survive.
    /// </summary>
    public static string Document(IEnumerable<ChatMessage> messages, string emptyStateHtml)
    {
        var body = new StringBuilder();
        int count = 0;
        foreach (ChatMessage m in messages)
        {
            body.Append(RenderMessage(m));
            count++;
        }
        string content = count > 0 ? body.ToString() : $"<div class=\"empty\">{emptyStateHtml}</div>";
        return Page.Replace("{{BODY}}", content);
    }

    /// <summary>A JS string literal for the given text, for ExecuteScriptAsync.</summary>
    public static string Js(string value) => JsonSerializer.Serialize(value);

    private static string HtmlAttr(string value) => WebUtility.HtmlEncode(value).Replace("\"", "&quot;");

    // The page itself. One committed dark theme — this panel lives inside a
    // dark tool window, so a light variant would never be seen.
    private const string Page = """
        <!doctype html>
        <html lang="en">
        <head>
        <meta charset="utf-8">
        <meta name="viewport" content="width=device-width, initial-scale=1">
        <style>
        :root {
          --graphite: #14161a;
          --slate:    #1b1f26;
          --rail:     #2a3038;
          --ink:      #e6e9ed;
          --muted:    #8a93a1;
          --amber:    #e8a33d;   /* the device side: hardware, layout, selection */
          --cyan:     #4fc3e8;   /* the software side: Jack, links, code */
          --red:       #ef6b62;
          --mono: "Cascadia Mono", Consolas, "Courier New", monospace;
          --sans: "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif;
          --display: "Segoe UI Variable Display", "Segoe UI Semibold", "Segoe UI", sans-serif;
        }
        * { box-sizing: border-box; }
        html, body { margin: 0; padding: 0; }
        body {
          background: var(--graphite);
          color: var(--ink);
          font-family: var(--sans);
          font-size: 13.5px;
          line-height: 1.6;
          -webkit-font-smoothing: antialiased;
        }
        #log { padding: 14px 16px 28px; }

        /* ---- a turn ---------------------------------------------------- */
        .turn { position: relative; padding: 0 0 4px 14px; margin: 0 0 22px; }
        .turn::before {
          content: ""; position: absolute; left: 0; top: 3px; bottom: 6px; width: 2px;
          background: var(--rail);
        }
        .turn.you::before  { background: var(--amber); }
        .turn.jack::before { background: var(--cyan); }
        .turn.failed::before { background: var(--red); }

        .turn > header { display: flex; align-items: baseline; gap: 8px; margin-bottom: 6px; }
        .eyebrow {
          font-family: var(--mono); font-size: 9.5px; letter-spacing: .14em;
          text-transform: uppercase; color: var(--muted);
        }
        .you .eyebrow  { color: var(--amber); }
        .jack .eyebrow { color: var(--cyan); }
        .byline { font-family: var(--mono); font-size: 9.5px; color: #5d6674; }
        .turn > header time { margin-left: auto; font-family: var(--mono); font-size: 9.5px; color: #5d6674; }

        /* The carrier: on only while a reply is streaming. */
        .carrier {
          width: 26px; height: 2px; background: var(--cyan); border-radius: 1px;
          transform-origin: left center; animation: carrier 1.1s ease-in-out infinite;
        }
        @keyframes carrier { 0%,100% { transform: scaleX(.25); opacity: .45; } 50% { transform: scaleX(1); opacity: 1; } }
        @media (prefers-reduced-motion: reduce) { .carrier { animation: none; opacity: .8; } }

        /* ---- prose ----------------------------------------------------- */
        .body > *:first-child { margin-top: 0; }
        .body > *:last-child { margin-bottom: 0; }
        .body p { margin: 0 0 10px; }
        .body h1, .body h2, .body h3, .body h4 {
          font-family: var(--display); font-weight: 600; line-height: 1.25;
          margin: 18px 0 8px; letter-spacing: -.01em;
        }
        .body h1 { font-size: 19px; }
        .body h2 { font-size: 16px; }
        .body h3 { font-size: 14.5px; }
        .body h4 { font-size: 13.5px; color: var(--muted); }
        .body ul, .body ol { margin: 0 0 10px; padding-left: 20px; }
        .body li { margin: 3px 0; }
        .body li::marker { color: var(--muted); }
        .body a { color: var(--cyan); text-decoration: none; border-bottom: 1px solid #2f5f70; }
        .body a:hover { border-bottom-color: var(--cyan); }
        .body a:focus-visible, .act:focus-visible { outline: 2px solid var(--cyan); outline-offset: 2px; }
        .body strong { color: #fff; font-weight: 600; }
        .body hr { border: 0; border-top: 1px solid var(--rail); margin: 16px 0; }
        .body blockquote {
          margin: 10px 0; padding: 2px 0 2px 12px;
          border-left: 2px solid var(--rail); color: var(--muted);
        }
        .body code {
          font-family: var(--mono); font-size: 12px;
          color: var(--amber); background: #21252c; padding: 1px 4px; border-radius: 2px;
        }

        /* ---- tables: readouts, so mono and hairlines ------------------- */
        .body table { border-collapse: collapse; width: 100%; margin: 0 0 12px; font-size: 12.5px; }
        .body thead th {
          font-family: var(--mono); font-size: 9.5px; letter-spacing: .1em; text-transform: uppercase;
          color: var(--muted); text-align: left; font-weight: 500;
          padding: 4px 10px 4px 0; border-bottom: 1px solid var(--rail);
        }
        .body tbody td { padding: 5px 10px 5px 0; border-bottom: 1px solid #21252c; vertical-align: top; }
        .body tbody tr:last-child td { border-bottom: 0; }
        .body td code { background: none; padding: 0; }

        /* ---- media ----------------------------------------------------- */
        .body img, .body video { max-width: 100%; height: auto; display: block;
          border: 1px solid var(--rail); border-radius: 2px; margin: 4px 0 12px; }
        .body audio { width: 100%; margin: 4px 0 12px; }

        /* ---- code cards ------------------------------------------------ */
        figure.code {
          margin: 0 0 12px; border: 1px solid var(--rail); border-radius: 3px;
          background: #16191f; overflow: hidden;
        }
        figure.code figcaption {
          display: flex; align-items: center; gap: 8px;
          padding: 5px 8px; background: #1b1f26; border-bottom: 1px solid var(--rail);
        }
        .chip {
          font-family: var(--mono); font-size: 9.5px; letter-spacing: .12em;
          color: var(--muted); text-transform: uppercase;
        }
        .code-xml .chip { color: var(--amber); }
        .acts { margin-left: auto; display: flex; gap: 4px; }
        .act {
          font-family: var(--sans); font-size: 11px; color: var(--muted);
          background: transparent; border: 1px solid var(--rail); border-radius: 2px;
          padding: 2px 8px; cursor: pointer;
        }
        .act:hover { color: var(--ink); border-color: #3d4550; }
        .act.primary { color: var(--amber); border-color: #5a4526; }
        .act.primary:hover { background: #2a2114; }
        .act.done { color: var(--cyan); border-color: #2f5f70; }
        figure.code pre { margin: 0; padding: 10px 12px; overflow-x: auto; }
        figure.code code { font-family: var(--mono); font-size: 12px; line-height: 1.55;
          color: #d5dae1; background: none; padding: 0; }

        .t-cmt   { color: #6a7482; font-style: italic; }
        .t-str   { color: #b6d99b; }
        .t-num   { color: #dfc184; }
        .t-kw    { color: #7fb2ea; }
        .t-type  { color: #6fd3c0; }
        .t-attr  { color: var(--amber); }
        .t-tag   { color: #7fb2ea; }
        .t-key   { color: var(--amber); }
        .t-punct { color: #6a7482; }

        /* ---- tool pills ------------------------------------------------ */
        .tools { display: flex; flex-wrap: wrap; gap: 4px; margin-bottom: 6px; }
        .tools:empty { display: none; }
        .tool {
          font-family: var(--mono); font-size: 9.5px; letter-spacing: .04em;
          color: var(--cyan); border: 1px solid #24404b; background: #172227;
          border-radius: 999px; padding: 1px 8px;
        }
        .tool::before { content: "\25CF"; font-size: 6px; vertical-align: middle; margin-right: 4px; opacity: .7; }

        /* ---- attachments ----------------------------------------------- */
        .files { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 8px; }
        .thumb img { max-width: 150px; max-height: 110px; margin: 0; border-radius: 2px; }
        .doc {
          display: inline-flex; align-items: baseline; gap: 6px;
          font-family: var(--mono); font-size: 11px; color: var(--ink);
          border: 1px solid var(--rail); border-radius: 2px; padding: 3px 8px; text-decoration: none;
        }
        .doc span { color: var(--muted); font-size: 9.5px; }
        .doc:hover { border-color: var(--amber); }

        /* ---- empty state ----------------------------------------------- */
        .empty { padding: 8px 0 0 14px; color: var(--muted); }
        .empty h2 {
          font-family: var(--display); font-size: 17px; font-weight: 600;
          color: var(--ink); margin: 0 0 6px; letter-spacing: -.01em;
        }
        .empty p { margin: 0 0 10px; }
        .empty ul { margin: 0; padding-left: 18px; }
        .empty li { margin: 4px 0; }
        .empty code { font-family: var(--mono); font-size: 11.5px; color: var(--amber); }

        ::-webkit-scrollbar { width: 10px; height: 10px; }
        ::-webkit-scrollbar-track { background: var(--graphite); }
        ::-webkit-scrollbar-thumb { background: #2f353e; border: 2px solid var(--graphite); border-radius: 5px; }
        ::-webkit-scrollbar-thumb:hover { background: #3d4550; }
        </style>
        </head>
        <body>
        <div id="log">{{BODY}}</div>
        <script>
        const log = document.getElementById('log');
        let pinned = true;

        // Follow the tail only while the reader is already at the tail, so
        // scrolling up to read does not get yanked back by the next token.
        addEventListener('scroll', () => {
          pinned = (innerHeight + scrollY) >= document.body.offsetHeight - 40;
        });
        function tail() { if (pinned) { scrollTo(0, document.body.scrollHeight); } }

        window.rn = {
          append(html) {
            const empty = log.querySelector('.empty');
            if (empty) { empty.remove(); }
            log.insertAdjacentHTML('beforeend', html);
            pinned = true; tail();
          },
          setLive(html) {
            const live = document.getElementById('live');
            if (live) { live.outerHTML = html; } else { this.append(html); }
            tail();
          },
          setBody(html) {
            const live = document.getElementById('live');
            if (!live) { return; }
            live.querySelector('.body').innerHTML = html;
            tail();
          },
          setTools(html) {
            const live = document.getElementById('live');
            if (live) { live.querySelector('.tools').innerHTML = html; }
          },
          settle() {
            const live = document.getElementById('live');
            if (!live) { return; }
            live.removeAttribute('id');
            const c = live.querySelector('.carrier');
            if (c) { c.remove(); }
          },
          send(action, code, lang) {
            chrome.webview.postMessage({ action, code, lang: lang || '' });
          }
        };

        function codeOf(figure) {
          const bytes = Uint8Array.from(atob(figure.dataset.code), c => c.charCodeAt(0));
          return new TextDecoder().decode(bytes);
        }

        addEventListener('click', e => {
          const button = e.target.closest('.act');
          if (button) {
            const figure = button.closest('figure.code');
            const code = codeOf(figure);
            const act = button.dataset.act;
            if (act === 'copy') {
              navigator.clipboard.writeText(code).then(() => {
                const was = button.textContent;
                button.textContent = 'Copied';
                button.classList.add('done');
                setTimeout(() => { button.textContent = was; button.classList.remove('done'); }, 1200);
              });
            } else {
              rn.send(act, code, figure.dataset.lang);
            }
            return;
          }
          // External links open in the real browser, not inside the panel.
          const a = e.target.closest('a');
          if (a && a.href && !a.href.startsWith('about:')) {
            e.preventDefault();
            rn.send('open-url', a.href);
          }
        });
        tail();
        </script>
        </body>
        </html>
        """;
}
