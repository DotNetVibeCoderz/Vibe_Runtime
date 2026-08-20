using System.ComponentModel;
using System.Net.Http.Json;
using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;
using CodeGen.Models;
using Microsoft.SemanticKernel;

namespace CodeGen.Plugins;

/// <summary>
/// Reaching outside the workspace: web search and page reading.
///
/// Both tools return plain text. The model does not need markup, and stripping
/// it early keeps a single page from swallowing the context window.
/// </summary>
public sealed partial class WebPlugin(AppSettings settings, HttpClient http, Action<string> log)
{
    /// <summary>Cap on the text returned from one page.</summary>
    private const int MaxPageCharacters = 12_000;

    [KernelFunction("search_internet")]
    [Description("Search the web and return the top results with a short summary of each. Use this for current information, library versions and API documentation.")]
    public async Task<string> SearchInternetAsync(
        [Description("What to search for.")] string query,
        [Description("How many results to return, 1 to 10.")] int maxResults = 5,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(query)) return "Give me something to search for.";
        if (string.IsNullOrWhiteSpace(settings.TavilyApiKey))
        {
            return "Web search needs a Tavily API key. Add it under Settings → Tools.";
        }

        maxResults = Math.Clamp(maxResults, 1, 10);
        log($"Jack searched the web for \"{query}\"");

        try
        {
            using var response = await http.PostAsJsonAsync(
                "https://api.tavily.com/search",
                new
                {
                    api_key = settings.TavilyApiKey,
                    query,
                    max_results = maxResults,
                    search_depth = "basic",
                    include_answer = true,
                },
                cancellationToken).ConfigureAwait(false);

            if (!response.IsSuccessStatusCode)
            {
                return $"Search failed: {(int)response.StatusCode} {response.ReasonPhrase}.";
            }

            var payload = await response.Content.ReadFromJsonAsync<JsonElement>(cancellationToken)
                .ConfigureAwait(false);

            var builder = new StringBuilder();
            if (payload.TryGetProperty("answer", out var answer)
                && answer.ValueKind == JsonValueKind.String
                && !string.IsNullOrWhiteSpace(answer.GetString()))
            {
                builder.AppendLine($"Summary: {answer.GetString()}").AppendLine();
            }

            if (payload.TryGetProperty("results", out var results)
                && results.ValueKind == JsonValueKind.Array)
            {
                var index = 1;
                foreach (var result in results.EnumerateArray())
                {
                    var title = Text(result, "title");
                    var url = Text(result, "url");
                    var content = Text(result, "content");
                    builder.AppendLine($"{index}. {title}");
                    builder.AppendLine($"   {url}");
                    if (content.Length > 0) builder.AppendLine($"   {Shorten(content, 400)}");
                    builder.AppendLine();
                    index++;
                }
            }

            return builder.Length == 0 ? "No results." : builder.ToString();
        }
        catch (Exception ex)
        {
            return $"Search failed: {ex.Message}";
        }
    }

    [KernelFunction("scrape_web_page")]
    [Description("Fetch a web page and return its readable text, with markup removed. Use this after search_internet to read a promising result in full.")]
    public async Task<string> ScrapeWebPageAsync(
        [Description("The full URL to fetch.")] string url,
        CancellationToken cancellationToken = default)
    {
        if (!Uri.TryCreate(url, UriKind.Absolute, out var uri)
            || (uri.Scheme != Uri.UriSchemeHttp && uri.Scheme != Uri.UriSchemeHttps))
        {
            return $"'{url}' is not an http or https URL.";
        }

        log($"Jack read {uri.Host}");

        try
        {
            using var response = await http.GetAsync(uri, cancellationToken).ConfigureAwait(false);
            if (!response.IsSuccessStatusCode)
            {
                return $"Could not fetch the page: {(int)response.StatusCode} {response.ReasonPhrase}.";
            }

            var media = response.Content.Headers.ContentType?.MediaType ?? "";
            var body = await response.Content.ReadAsStringAsync(cancellationToken).ConfigureAwait(false);

            // Plain text, JSON and source files are already readable.
            var text = media.Contains("html", StringComparison.OrdinalIgnoreCase)
                ? ExtractText(body)
                : body;

            return Shorten(text, MaxPageCharacters);
        }
        catch (Exception ex)
        {
            return $"Could not fetch the page: {ex.Message}";
        }
    }

    /// <summary>
    /// Reduces HTML to its readable text.
    ///
    /// A real parser would be better, but the goal here is only to feed the
    /// model prose: drop the parts that are never prose, unwrap the rest, and
    /// collapse the whitespace that markup leaves behind.
    /// </summary>
    internal static string ExtractText(string html)
    {
        var withoutScripts = ScriptOrStyle().Replace(html, " ");
        var withoutComments = HtmlComment().Replace(withoutScripts, " ");
        // Turn block boundaries into newlines so paragraphs survive.
        var withBreaks = BlockBoundary().Replace(withoutComments, "\n");
        var withoutTags = HtmlTag().Replace(withBreaks, " ");
        var decoded = System.Net.WebUtility.HtmlDecode(withoutTags);

        var lines = decoded
            .Split('\n')
            .Select(line => CollapseSpaces().Replace(line, " ").Trim())
            .Where(line => line.Length > 0);

        return string.Join('\n', lines);
    }

    private static string Text(JsonElement element, string property) =>
        element.TryGetProperty(property, out var value) && value.ValueKind == JsonValueKind.String
            ? value.GetString() ?? ""
            : "";

    private static string Shorten(string text, int limit) =>
        text.Length <= limit ? text : $"{text[..limit]}\n… truncated at {limit} characters.";

    [GeneratedRegex(@"<(script|style)\b[^>]*>.*?</\1>", RegexOptions.Singleline | RegexOptions.IgnoreCase)]
    private static partial Regex ScriptOrStyle();

    [GeneratedRegex(@"<!--.*?-->", RegexOptions.Singleline)]
    private static partial Regex HtmlComment();

    [GeneratedRegex(@"</?(p|div|br|li|tr|h[1-6]|section|article|header|footer)\b[^>]*>", RegexOptions.IgnoreCase)]
    private static partial Regex BlockBoundary();

    [GeneratedRegex(@"<[^>]+>")]
    private static partial Regex HtmlTag();

    [GeneratedRegex(@"[ \t\f\v ]+")]
    private static partial Regex CollapseSpaces();
}
