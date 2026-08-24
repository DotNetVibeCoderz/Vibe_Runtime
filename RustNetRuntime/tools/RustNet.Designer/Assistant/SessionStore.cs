using System;
using System.Collections.Generic;
using System.IO;
using System.Text.Json;

namespace RustNet.Designer.Assistant;

/// <summary>
/// Sessions on disk: one JSON file per conversation under
/// <c>&lt;DataDirectory&gt;/sessions</c>, attachments under
/// <c>&lt;DataDirectory&gt;/attachments/&lt;sessionId&gt;</c>. Deleting a
/// session takes its attachments with it.
/// </summary>
public sealed class SessionStore
{
    private static readonly JsonSerializerOptions Json = new()
    {
        WriteIndented = true,
        Converters = { new System.Text.Json.Serialization.JsonStringEnumConverter() },
    };

    private readonly string _root;

    public SessionStore(string dataDirectory)
    {
        _root = dataDirectory;
        Directory.CreateDirectory(SessionsDir);
        Directory.CreateDirectory(AttachmentsDir);
    }

    public string SessionsDir => Path.Combine(_root, "sessions");
    public string AttachmentsDir => Path.Combine(_root, "attachments");

    public string AttachmentDirFor(ChatSession session)
    {
        string dir = Path.Combine(AttachmentsDir, session.Id);
        Directory.CreateDirectory(dir);
        return dir;
    }

    private string PathFor(string id) => Path.Combine(SessionsDir, id + ".json");

    /// <summary>All sessions, newest activity first. Unreadable files are skipped.</summary>
    public List<ChatSession> LoadAll()
    {
        var list = new List<ChatSession>();
        foreach (string file in Directory.EnumerateFiles(SessionsDir, "*.json"))
        {
            try
            {
                ChatSession? s = JsonSerializer.Deserialize<ChatSession>(File.ReadAllText(file), Json);
                if (s != null && s.Id.Length > 0)
                {
                    list.Add(s);
                }
            }
            catch (Exception ex)
            {
                System.Diagnostics.Debug.WriteLine($"skipping unreadable session {file}: {ex.Message}");
            }
        }
        list.Sort((a, b) => b.UpdatedUtc.CompareTo(a.UpdatedUtc));
        return list;
    }

    public void Save(ChatSession session)
    {
        session.UpdatedUtc = DateTime.UtcNow;
        File.WriteAllText(PathFor(session.Id), JsonSerializer.Serialize(session, Json));
    }

    public void Delete(ChatSession session)
    {
        string file = PathFor(session.Id);
        if (File.Exists(file))
        {
            File.Delete(file);
        }
        string dir = Path.Combine(AttachmentsDir, session.Id);
        if (Directory.Exists(dir))
        {
            Directory.Delete(dir, recursive: true);
        }
    }

    /// <summary>
    /// Empty a session in place — same id, same folder, no transcript. Keeping
    /// the id means the person's place in the session list does not move.
    /// </summary>
    public void Reset(ChatSession session)
    {
        session.Messages.Clear();
        session.Title = "New session";
        string dir = Path.Combine(AttachmentsDir, session.Id);
        if (Directory.Exists(dir))
        {
            Directory.Delete(dir, recursive: true);
        }
        Save(session);
    }
}
