using System.IO;

namespace Conformance;

// File names a map takes in: an imported "kecamatan.geojson" becomes the layer
// "kecamatan". Only '/' paths, so the answers are the same on every OS.
// Expected values were produced by dotnet.
static partial class Program
{
    // Windows .NET writes  between directories; the runtime writes /.
    static string? Slash(string? p) => p?.Replace('\\', '/');

    static void PathChecks()
    {
        CheckStr("Path.GetFileNameWithoutExtension and GetFileName",
            $"{Path.GetFileNameWithoutExtension("data/kecamatan.geojson")}|{Path.GetFileNameWithoutExtension("report.v2.json")}|{Path.GetFileNameWithoutExtension("noext")}|{Path.GetFileName("a/b/c.csv")}|{Path.GetFileName((string?)null) is null}",
            "kecamatan|report.v2|noext|c.csv|True");
        CheckStr("Path.GetExtension and HasExtension",
            $"{Path.GetExtension("a.tar.gz")}|[{Path.GetExtension("dot.")}]|{Path.GetExtension(".gitignore")}|{Path.HasExtension("x.json")}|{Path.HasExtension("x")}",
            ".gz|[]|.gitignore|True|False");
        CheckStr("Path.GetDirectoryName, ChangeExtension, Combine and Join",
            $"{Slash(Path.GetDirectoryName("a/b/c.txt"))}|{Path.GetDirectoryName("c.txt")}|{Path.ChangeExtension("a/b.json", ".geojson")}|{Path.ChangeExtension("b", "csv")}|{Slash(Path.Combine("data", "maps", "a.json"))}|{Path.Combine("data", "/abs")}|{Path.Join("data", "/abs")}",
            "a/b||a/b.geojson|b.csv|data/maps/a.json|/abs|data/abs");
    }
}
