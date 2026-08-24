using System;
using System.ComponentModel;
using System.Net.Http;
using System.Net.Http.Headers;
using System.Text;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.SemanticKernel;

namespace RustNet.Designer.Assistant.Plugins;

/// <summary>
/// Reaching outside the machine: internet search through Tavily, and fetching a
/// page or a file by URL. Every result is capped at
/// <c>Tools.Http.MaxChars</c> so one large page cannot crowd out the
/// conversation, and failures come back as readable text rather than
/// exceptions — a failed tool call should not end the turn.
/// </summary>
public sealed class WebPlugin : IDisposable
{
    private readonly AssistantOptions _options;
    private readonly HttpClient _http;

    public WebPlugin(AssistantOptions options)
    {
        _options = options;
        _http = new HttpClient { Timeout = TimeSpan.FromSeconds(options.HttpTimeoutSeconds) };
        _http.DefaultRequestHeaders.UserAgent.ParseAdd("RustNet-Designer-Assistant/1.0");
    }

    [KernelFunction("search_web")]
    [Description("Search the internet and get back titles, URLs and content snippets. Use it for "
        + "datasheets, part numbers, protocol details and anything else that is not in this repo.")]
    public async Task<string> SearchWebAsync(
        [Description("What to search for. Plain words work better than a question.")] string query,
        [Description("How many results to return, 1-10. Defaults to the configured value.")] int maxResults = 0,
        CancellationToken cancellationToken = default)
    {
        if (_options.TavilyApiKey.Length == 0)
        {
            return "Search is not configured. Set Tools.Tavily.ApiKey in app.config, or export "
                + "TAVILY_API_KEY. Tell the person that, and answer from what you already know.";
        }
        int count = maxResults > 0 ? Math.Clamp(maxResults, 1, 10) : _options.TavilyMaxResults;

        var payload = new
        {
            query,
            max_results = count,
            search_depth = "advanced",
            include_answer = true,
        };

        try
        {
            using var request = new HttpRequestMessage(HttpMethod.Post, "https://api.tavily.com/search");
            request.Headers.Authorization = new AuthenticationHeaderValue("Bearer", _options.TavilyApiKey);
            request.Content = new StringContent(JsonSerializer.Serialize(payload), Encoding.UTF8, "application/json");

            using HttpResponseMessage response = await _http.SendAsync(request, cancellationToken).ConfigureAwait(false);
            string body = await response.Content.ReadAsStringAsync(cancellationToken).ConfigureAwait(false);
            if (!response.IsSuccessStatusCode)
            {
                return $"Search failed ({(int)response.StatusCode} {response.ReasonPhrase}): {Trim(body, 500)}";
            }
            return FormatTavily(body, query);
        }
        catch (Exception ex)
        {
            return "Search failed: " + ex.Message;
        }
    }

    [KernelFunction("fetch_page")]
    [Description("Fetch a web page and return its readable text with the markup stripped. "
        + "Use it to read a page a search result pointed at.")]
    public async Task<string> FetchPageAsync(
        [Description("Absolute http(s) URL.")] string url,
        CancellationToken cancellationToken = default)
    {
        string raw = await GetAsync(url, cancellationToken).ConfigureAwait(false);
        if (raw.StartsWith("Fetch failed", StringComparison.Ordinal))
        {
            return raw;
        }
        string text = HtmlToText.Convert(raw);
        return Cap($"# {url}\n\n{text}");
    }

    [KernelFunction("fetch_file")]
    [Description("Fetch a file by URL and return its contents verbatim, without stripping markup. "
        + "Use it for raw source, JSON, CSV, XML or a plain-text datasheet.")]
    public async Task<string> FetchFileAsync(
        [Description("Absolute http(s) URL.")] string url,
        CancellationToken cancellationToken = default)
    {
        string raw = await GetAsync(url, cancellationToken).ConfigureAwait(false);
        return raw.StartsWith("Fetch failed", StringComparison.Ordinal) ? raw : Cap(raw);
    }

    private async Task<string> GetAsync(string url, CancellationToken cancellationToken)
    {
        if (!Uri.TryCreate(url, UriKind.Absolute, out Uri? uri)
            || (uri.Scheme != Uri.UriSchemeHttp && uri.Scheme != Uri.UriSchemeHttps))
        {
            return $"Fetch failed: {url} is not an http(s) URL.";
        }
        try
        {
            using HttpResponseMessage response = await _http.GetAsync(uri, cancellationToken).ConfigureAwait(false);
            if (!response.IsSuccessStatusCode)
            {
                return $"Fetch failed: {(int)response.StatusCode} {response.ReasonPhrase} for {url}";
            }
            return await response.Content.ReadAsStringAsync(cancellationToken).ConfigureAwait(false);
        }
        catch (Exception ex)
        {
            return $"Fetch failed: {ex.Message}";
        }
    }

    private static string FormatTavily(string json, string query)
    {
        try
        {
            using JsonDocument doc = JsonDocument.Parse(json);
            var sb = new StringBuilder();
            sb.AppendLine($"# Search: {query}");
            if (doc.RootElement.TryGetProperty("answer", out JsonElement answer)
                && answer.ValueKind == JsonValueKind.String
                && answer.GetString() is { Length: > 0 } summary)
            {
                sb.AppendLine().AppendLine("Summary: " + summary);
            }
            if (doc.RootElement.TryGetProperty("results", out JsonElement results)
                && results.ValueKind == JsonValueKind.Array)
            {
                int n = 0;
                foreach (JsonElement r in results.EnumerateArray())
                {
                    n++;
                    sb.AppendLine();
                    sb.AppendLine($"## {n}. {Text(r, "title")}");
                    sb.AppendLine(Text(r, "url"));
                    string content = Text(r, "content");
                    if (content.Length > 0)
                    {
                        sb.AppendLine().AppendLine(Trim(content, 1500));
                    }
                }
                if (n == 0)
                {
                    sb.AppendLine().AppendLine("No results.");
                }
            }
            return sb.ToString();
        }
        catch (JsonException)
        {
            return Trim(json, 4000);
        }

        static string Text(JsonElement e, string name)
            => e.TryGetProperty(name, out JsonElement v) && v.ValueKind == JsonValueKind.String
                ? v.GetString() ?? "" : "";
    }

    private string Cap(string s)
        => s.Length <= _options.HttpMaxChars
            ? s
            : s.Substring(0, _options.HttpMaxChars) + $"\n\n[truncated at {_options.HttpMaxChars} characters]";

    private static string Trim(string s, int max)
        => s.Length <= max ? s : s.Substring(0, max) + "…";

    public void Dispose() => _http.Dispose();
}
