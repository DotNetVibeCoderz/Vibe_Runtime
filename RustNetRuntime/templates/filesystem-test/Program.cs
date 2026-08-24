using RustNet.IO;

namespace __NAME__;

/// <summary>Filesystem smoke test: directories, write, append, read, list, delete.</summary>
public static class Program
{
    private static int _passed;
    private static int _failed;

    public static void Main()
    {
        Console.WriteLine("__NAME__ filesystem test");

        FileSystem.CreateDirectory("/data/fstest");
        Check("exists after mkdir", FileSystem.Exists("/data/fstest"));

        FileSystem.WriteAllText("/data/fstest/a.txt", "hello");
        Check("write+read", FileSystem.ReadAllText("/data/fstest/a.txt") == "hello");

        FileSystem.AppendText("/data/fstest/a.txt", " world");
        Check("append", FileSystem.ReadAllText("/data/fstest/a.txt") == "hello world");

        FileSystem.WriteAllText("/data/fstest/b.txt", "second");
        // `List` returns the names, not a joined blob: a caller that wants one
        // string can join it, and a caller that wants the entries cannot split
        // one reliably when a name contains the separator.
        string listing = string.Join(",", FileSystem.List("/data/fstest"));
        Check("list contains a.txt", listing.Contains("a.txt"));
        Check("list contains b.txt", listing.Contains("b.txt"));

        FileSystem.Delete("/data/fstest/a.txt");
        Check("delete", !FileSystem.Exists("/data/fstest/a.txt"));
        Check("b still exists", FileSystem.Exists("/data/fstest/b.txt"));

        // Overwrite semantics
        FileSystem.WriteAllText("/data/fstest/b.txt", "overwritten");
        Check("overwrite", FileSystem.ReadAllText("/data/fstest/b.txt") == "overwritten");

        Console.WriteLine(string.Concat("passed: ", _passed.ToString(), ", failed: ", _failed.ToString()));
        if (_failed == 0)
        {
            Console.WriteLine("FILESYSTEM TEST OK");
        }
    }

    private static void Check(string name, bool ok)
    {
        if (ok)
        {
            _passed = _passed + 1;
            Console.WriteLine(string.Concat("  ok: ", name));
        }
        else
        {
            _failed = _failed + 1;
            Console.WriteLine(string.Concat("  FAIL: ", name));
        }
    }
}
