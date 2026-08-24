using System;
using System.Collections.Generic;
using System.IO;
using System.Text;

namespace RustNet.Designer.Assistant;

/// <summary>
/// "Uploading" an attachment: the file is copied into the session's own folder
/// and gets a stable <c>file:///</c> URL. That URL is what the transcript
/// renders (so images show inline and documents are clickable) and what the
/// model is told about.
///
/// Images additionally travel to the model as image content — the bytes, not
/// the URL, because a hosted model cannot reach a path on this machine.
/// Documents travel as a link plus an inlined text excerpt; the rest is
/// available to the model through the <c>read_attachment</c> function.
/// </summary>
public sealed class AttachmentStore
{
    /// <summary>
    /// The virtual host the transcript and its attachments are served from.
    /// WebView2 maps it to the assistant's data directory, which gives the page
    /// a real https origin — needed for the clipboard API, and the only way an
    /// attached image can be shown at all.
    /// </summary>
    public const string VirtualHost = "rustnet.assets";

    private static readonly HashSet<string> ImageExtensions = new(StringComparer.OrdinalIgnoreCase)
    {
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp",
    };

    // Extensions we can read as text. Anything else is attached as a link only.
    private static readonly HashSet<string> TextExtensions = new(StringComparer.OrdinalIgnoreCase)
    {
        ".txt", ".md", ".markdown", ".cs", ".xml", ".json", ".yaml", ".yml", ".toml",
        ".csv", ".log", ".rs", ".c", ".h", ".cpp", ".hpp", ".py", ".js", ".ts",
        ".html", ".css", ".ini", ".cfg", ".config", ".sql", ".sh", ".ps1", ".csproj",
    };

    private readonly SessionStore _store;
    private readonly AssistantOptions _options;

    public AttachmentStore(SessionStore store, AssistantOptions options)
    {
        _store = store;
        _options = options;
    }

    public static bool LooksLikeImage(string path) => ImageExtensions.Contains(Path.GetExtension(path));

    /// <summary>
    /// Copy <paramref name="sourcePath"/> into the session folder and describe
    /// it. Throws <see cref="IOException"/> when an image exceeds the
    /// configured size cap, because sending it would fail at the provider
    /// instead — better to say so at attach time.
    /// </summary>
    public ChatAttachment Add(ChatSession session, string sourcePath)
    {
        var info = new FileInfo(sourcePath);
        if (!info.Exists)
        {
            throw new FileNotFoundException("Attachment not found", sourcePath);
        }

        bool isImage = LooksLikeImage(sourcePath);
        if (isImage && info.Length > _options.MaxImageBytes)
        {
            throw new IOException(
                $"{info.Name} is {info.Length / 1024} KB; the image limit is {_options.MaxImageBytes / 1024} KB. "
                + "Raise Attachments.MaxImageBytes or scale the image down.");
        }

        string dir = _store.AttachmentDirFor(session);
        string stored = UniquePath(dir, info.Name);
        File.Copy(sourcePath, stored, overwrite: false);

        var att = new ChatAttachment
        {
            Kind = isImage ? AttachmentKind.Image : AttachmentKind.Document,
            FileName = Path.GetFileName(stored),
            StoredPath = stored,
            Url = new Uri(stored).AbsoluteUri,
            WebUrl = $"https://{VirtualHost}/attachments/{session.Id}/{Uri.EscapeDataString(Path.GetFileName(stored))}",
            MimeType = MimeFor(stored),
            SizeBytes = info.Length,
        };

        if (!isImage && TextExtensions.Contains(info.Extension))
        {
            string text = ReadTextSafely(stored);
            if (text.Length > _options.MaxDocumentChars)
            {
                att.TextExcerpt = text.Substring(0, _options.MaxDocumentChars);
                att.Truncated = true;
            }
            else
            {
                att.TextExcerpt = text;
            }
        }
        return att;
    }

    /// <summary>Read the stored bytes of an attachment (image content path).</summary>
    public static byte[] ReadBytes(ChatAttachment attachment) => File.ReadAllBytes(attachment.StoredPath);

    /// <summary>
    /// Whole text of a stored attachment, capped — the backing read for the
    /// <c>read_attachment</c> function.
    /// </summary>
    public string ReadFullText(ChatAttachment attachment)
    {
        string text = ReadTextSafely(attachment.StoredPath);
        return text.Length <= _options.HttpMaxChars ? text : text.Substring(0, _options.HttpMaxChars);
    }

    private static string ReadTextSafely(string path)
    {
        using var reader = new StreamReader(path, Encoding.UTF8, detectEncodingFromByteOrderMarks: true);
        return reader.ReadToEnd();
    }

    private static string UniquePath(string dir, string fileName)
    {
        string candidate = Path.Combine(dir, fileName);
        if (!File.Exists(candidate))
        {
            return candidate;
        }
        string stem = Path.GetFileNameWithoutExtension(fileName);
        string ext = Path.GetExtension(fileName);
        for (int n = 2; ; n++)
        {
            candidate = Path.Combine(dir, $"{stem}-{n}{ext}");
            if (!File.Exists(candidate))
            {
                return candidate;
            }
        }
    }

    public static string MimeFor(string path) => Path.GetExtension(path).ToLowerInvariant() switch
    {
        ".png" => "image/png",
        ".jpg" or ".jpeg" => "image/jpeg",
        ".gif" => "image/gif",
        ".webp" => "image/webp",
        ".bmp" => "image/bmp",
        ".svg" => "image/svg+xml",
        ".pdf" => "application/pdf",
        ".json" => "application/json",
        ".xml" or ".csproj" or ".config" => "text/xml",
        ".md" or ".markdown" => "text/markdown",
        ".csv" => "text/csv",
        ".html" => "text/html",
        ".txt" or ".log" or ".cs" or ".rs" or ".py" or ".ps1" or ".sh" => "text/plain",
        _ => "application/octet-stream",
    };
}
