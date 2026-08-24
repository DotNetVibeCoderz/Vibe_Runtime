using RustNet.Deploy;

namespace RndpCheck;

/// <summary>
/// What "the Workbench can talk to a device" means, as a program.
///
/// The Device fixture's sibling for the wire. It drives a real
/// <see cref="RndpClient"/> — the one the Workbench and the Deploy tool use —
/// against a running <c>rustnet serve</c>, so the thing under test is the
/// agreement between two independent implementations of RNDP: C# on this side,
/// Rust on the other.
///
/// That agreement cannot be checked by a unit test on either side. Both would
/// pass happily while disagreeing about, say, whether the length is
/// little-endian, and the first sign of trouble would be a board that ignores
/// a command.
///
/// Run it with:
///     rustnet serve --image device.img &amp;
///     dotnet tests/fixtures/RndpCheck/bin/Release/net10.0/RndpCheck.dll
/// </summary>
public static class Program
{
    static int checks;
    static int failures;

    static void Check(string name, bool ok)
    {
        checks++;
        if (!ok)
        {
            failures++;
            Console.WriteLine("FAIL " + name);
        }
    }

    static void CheckEq(string name, string actual, string expected)
    {
        checks++;
        if (actual != expected)
        {
            failures++;
            Console.WriteLine("FAIL " + name);
            Console.WriteLine($"     got      [{actual}]");
            Console.WriteLine($"     expected [{expected}]");
        }
    }

    /// <summary>
    /// Something shaped like an assembly: the service checks for the `MZ` a PE
    /// file starts with, and nothing beyond that until it is asked to run.
    /// </summary>
    static byte[] FakeAssembly(int size)
    {
        var image = new byte[size];
        image[0] = (byte)'M';
        image[1] = (byte)'Z';
        for (int i = 2; i < size; i++)
        {
            image[i] = (byte)(i & 0xFF);
        }
        return image;
    }

    public static int Main(string[] args)
    {
        string spec = args.Length > 0 ? args[0] : "tcp:127.0.0.1:7878";

        using var client = RndpClient.Connect(spec);

        // -- identity -------------------------------------------------------

        client.Ping();
        Check("a device answers PING", true);

        // INFO is JSON, and the field names are a contract: the Workbench
        // parses exactly these, and shows raw text when it cannot.
        string info = client.Info();
        using (var doc = System.Text.Json.JsonDocument.Parse(info))
        {
            var root = doc.RootElement;
            foreach (string field in new[]
                     { "chip", "board", "version", "protocol", "uptime_ms",
                       "heap_used", "apps", "wifi", "active_app", "running", "autostart" })
            {
                Check($"INFO carries `{field}`", root.TryGetProperty(field, out _));
            }
            Check("INFO reports protocol 1", root.GetProperty("protocol").GetInt32() == 1);
            Check("INFO names the chip", root.GetProperty("chip").GetString()!.Length > 0);
        }

        // -- applications ---------------------------------------------------

        // Start from a known state: the device keeps its flash between runs,
        // which is the whole point of it, so a second run must not trip over
        // what the first one left.
        // The listing is a JSON array and each entry names itself. Parsed
        // rather than pattern-matched, so adding a field does not break this.
        using (var listing = System.Text.Json.JsonDocument.Parse(client.ListApps()))
        {
            foreach (var app in listing.RootElement.EnumerateArray())
            {
                try { client.StopApp(); } catch { }
                client.EraseApp(app.GetProperty("name").GetString()!);
            }
        }
        CheckEq("the device starts with no applications", client.ListApps(), "[]");

        byte[] image = FakeAssembly(4096);
        client.FlashApp("demo", image);
        Check("a flashed application is listed",
            client.ListApps().Contains("\"name\":\"demo\"")
            && client.ListApps().Contains("\"size\":4096"));

        client.FlashApp("second", FakeAssembly(128));
        Check("a second application is listed too", client.ListApps().Contains("second"));

        // Newest wins: the store is append-only, so re-flashing has to shadow
        // the previous record rather than accumulate a second entry.
        client.FlashApp("demo", FakeAssembly(2048));
        using (var listing = System.Text.Json.JsonDocument.Parse(client.ListApps()))
        {
            Check("re-flashing does not duplicate the entry",
                listing.RootElement.GetArrayLength() == 2);
        }

        // -- what must be refused -------------------------------------------

        Check("a name containing a path is refused", Refused(() => client.FlashApp("../escape", image)));
        Check("something that is not a PE file is refused",
            Refused(() => client.FlashApp("html", "<html>"u8.ToArray())));
        Check("autostart pointing at nothing is refused", Refused(() => client.SetAutostart("ghost")));

        // -- autostart ------------------------------------------------------

        client.SetAutostart("demo");
        Check("autostart is reported in the listing",
            client.ListApps().Contains("\"autostart\":true"));
        client.SetAutostart(null);
        Check("autostart can be cleared",
            !client.ListApps().Contains("\"autostart\":true"));

        // -- data files -----------------------------------------------------

        client.FlashData("cfg/thresholds.txt", "hot=40\ncold=5"u8.ToArray());
        CheckEq("a data file reads back",
            System.Text.Encoding.UTF8.GetString(client.ReadData("cfg/thresholds.txt")),
            "hot=40\ncold=5");

        // -- diagnostics ----------------------------------------------------

        Check("PERF reports the flash it has used", client.GetPerf().Contains("flash_used="));
        client.GetLogs();
        Check("LOGS is readable even when empty", true);

        // -- unimplemented commands name themselves --------------------------

        Check("an unimplemented command is refused by name", Refused(() => client.OtaRollback()));

        // -- cleanup ---------------------------------------------------------

        client.EraseApp("second");
        client.EraseApp("demo");
        CheckEq("the device ends with no applications", client.ListApps(), "[]");

        Console.WriteLine($"rndp checks={checks} failures={failures}");
        return failures == 0 ? 0 : 1;
    }

    /// <summary>Whether the device refused, which for these cases is a pass.</summary>
    static bool Refused(Action body)
    {
        try
        {
            body();
            return false;
        }
        catch (Exception)
        {
            return true;
        }
    }
}
