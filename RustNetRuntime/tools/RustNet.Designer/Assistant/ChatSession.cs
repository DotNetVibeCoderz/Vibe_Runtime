using System;
using System.Collections.Generic;
using System.Text.Json.Serialization;

namespace RustNet.Designer.Assistant;

public enum ChatRole
{
    User,
    Assistant,
    System,
}

public enum AttachmentKind
{
    Image,
    Document,
}

/// <summary>
/// A file the person attached to a message. Uploading copies the file into the
/// session's attachment folder and records its <c>file:///</c> URL: images go
/// to the model as image content, documents go in as a link plus an inlined
/// text excerpt (the model can pull the rest with <c>read_attachment</c>).
/// </summary>
public sealed class ChatAttachment
{
    public AttachmentKind Kind { get; set; }
    public string FileName { get; set; } = "";
    /// <summary>Absolute path inside the session's attachment folder.</summary>
    public string StoredPath { get; set; } = "";
    /// <summary>The <c>file:///</c> URL — what the message text links to and what
    /// "open" hands to the shell.</summary>
    public string Url { get; set; } = "";
    /// <summary>
    /// The same file behind the transcript's virtual host. The transcript is a
    /// real https origin so the clipboard API and images work; a
    /// <c>file:///</c> src would be blocked from it.
    /// </summary>
    public string WebUrl { get; set; } = "";
    public string MimeType { get; set; } = "application/octet-stream";
    public long SizeBytes { get; set; }
    /// <summary>Extracted text for documents; empty for images and binaries.</summary>
    public string TextExcerpt { get; set; } = "";
    /// <summary>Set when the excerpt is a prefix of a longer document.</summary>
    public bool Truncated { get; set; }
}

/// <summary>One turn in the transcript.</summary>
public sealed class ChatMessage
{
    public ChatRole Role { get; set; }
    public string Text { get; set; } = "";
    public DateTime CreatedUtc { get; set; } = DateTime.UtcNow;
    public List<ChatAttachment> Attachments { get; set; } = new();
    /// <summary>Kernel functions the model called while producing this reply.</summary>
    public List<string> ToolCalls { get; set; } = new();
    /// <summary>Marks a reply that failed, so it renders as an error card.</summary>
    public bool IsError { get; set; }
    /// <summary>Provider/model that produced an assistant reply, for the byline.</summary>
    public string Model { get; set; } = "";
}

/// <summary>
/// A named conversation. Sessions are independent: each has its own transcript,
/// its own attachment folder, and remembers which model produced it.
/// </summary>
public sealed class ChatSession
{
    public string Id { get; set; } = Guid.NewGuid().ToString("n").Substring(0, 12);
    public string Title { get; set; } = "New session";
    public DateTime CreatedUtc { get; set; } = DateTime.UtcNow;
    public DateTime UpdatedUtc { get; set; } = DateTime.UtcNow;
    public string Provider { get; set; } = "";
    public string Model { get; set; } = "";
    public List<ChatMessage> Messages { get; set; } = new();

    [JsonIgnore]
    public bool IsEmpty => Messages.Count == 0;

    /// <summary>
    /// A session names itself from its first user message, so the list reads
    /// like a list of topics rather than a list of timestamps.
    /// </summary>
    public void RetitleFromFirstMessage()
    {
        foreach (ChatMessage m in Messages)
        {
            if (m.Role != ChatRole.User || m.Text.Trim().Length == 0)
            {
                continue;
            }
            string line = m.Text.Trim().Replace('\r', ' ').Replace('\n', ' ');
            int cut = line.IndexOf(". ", StringComparison.Ordinal);
            if (cut > 12)
            {
                line = line.Substring(0, cut);
            }
            Title = line.Length <= 48 ? line : line.Substring(0, 47).TrimEnd() + "…";
            return;
        }
        Title = "New session";
    }
}
