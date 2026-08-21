using CodeGen.Models;
using CodeGen.Services;

namespace CodeGen;

/// <summary>
/// Scaffolds every template, builds it, and runs it on both runtimes.
///
/// This exists because of a claim the templates make and nothing checked:
/// <c>RunsOnRustClr = true</c> is a property in a record, and a template can
/// carry it while using a feature RustCLR declines. The repository convention
/// says templates marked that way "must actually run there" — so this is what
/// turns the convention into a command.
///
/// It also checks something subtler. Templates with
/// <see cref="BclTier.Minimal"/> promise to stay inside the bindings a small
/// board can hold, and the only honest way to know is to run them against a
/// runtime that has only those bindings. <c>rustnet run --bcl minimal</c> does
/// that on a desktop, so a template can be checked without a board.
/// </summary>
public static class TemplateVerifier
{
    public static int Run(string? filter, bool keepArtifacts)
    {
        var runner = new ProcessRunner();
        var root = Path.Combine(
            Path.GetTempPath(),
            "codegen-template-verify-" + DateTime.UtcNow.ToString("yyyyMMddHHmmss"));
        Directory.CreateDirectory(root);

        var settings = new AppSettings();
        var rustnet = BuildService.ResolveRustNet(settings);

        var templates = TemplateCatalog.All
            .Where(t => filter is null || t.Id.Contains(filter, StringComparison.OrdinalIgnoreCase))
            .ToList();

        Console.WriteLine($"Verifying {templates.Count} template(s) in {root}");
        Console.WriteLine();
        Console.WriteLine($"{"TEMPLATE",-22} {"TIER",-8} {"BUILD",-7} {"DOTNET",-7} {"RUSTCLR",-8} RESULT");
        Console.WriteLine(new string('-', 78));

        var failures = 0;
        var skipped = 0;

        foreach (var template in templates)
        {
            // Web, Desktop and Mobile templates need a host or a device to run,
            // and a Library has no entry point to run at all. Building them is
            // the whole check available here, and pretending otherwise would
            // make this report meaningless — the first version of this reported
            // the library template as FAIL when it had simply been asked to
            // execute a .dll with no `Main`.
            var runnable = template.Category is TemplateCategory.Console
                or TemplateCategory.IoT;

            var projects = new ProjectService();
            try
            {
                // `Create` returns the first source file it wrote; the project
                // directory is what it opened.
                projects.Create(root, "T" + template.Id.Replace("-", ""), template);
            }
            catch (Exception e)
            {
                Report(template, "-", "SCAFFOLD", "-", "-", e.Message);
                failures++;
                continue;
            }

            var directory = projects.CurrentProjectPath!;
            var target = projects.FindBuildTarget();
            if (target is null)
            {
                Report(template, "-", "no csproj", "-", "-", "template wrote no project file");
                failures++;
                continue;
            }

            var build = runner
                .RunAsync("dotnet", ["build", "-c", "Release", "--nologo", "-v", "q", target], directory)
                .GetAwaiter().GetResult();
            if (!build.Succeeded)
            {
                Report(template, TierOf(template), "FAIL", "-", "-", FirstError(build.Combined));
                failures++;
                continue;
            }

            if (!runnable)
            {
                Report(template, TierOf(template), "ok", "skip", "skip",
                    template.Category == TemplateCategory.Library
                        ? "library — no entry point"
                        : "needs a host to run");
                skipped++;
                continue;
            }

            var assembly = projects.FindOutputAssembly("Release");
            if (assembly is null)
            {
                Report(template, TierOf(template), "ok", "-", "-", "no output assembly");
                failures++;
                continue;
            }

            var onDotnet = runner.RunAsync("dotnet", [assembly], directory).GetAwaiter().GetResult();
            if (!onDotnet.Succeeded)
            {
                Report(template, TierOf(template), "ok", "FAIL", "-", FirstError(onDotnet.Combined));
                failures++;
                continue;
            }

            // The tier a template claims decides which runtime it is checked
            // against. A `Minimal` template checked against the full binding
            // set would pass while still being unable to run on the board it
            // was written for.
            List<string> arguments = ["run", assembly];
            if (template.MinimumTier == BclTier.Minimal)
            {
                arguments.AddRange(["--bcl", "minimal"]);
            }
            var onRustClr = runner.RunAsync(rustnet, arguments, directory).GetAwaiter().GetResult();

            if (!onRustClr.Succeeded)
            {
                if (template.RunsOnRustClr)
                {
                    Report(template, TierOf(template), "ok", "ok", "FAIL", FirstError(onRustClr.Combined));
                    failures++;
                }
                else
                {
                    Report(template, TierOf(template), "ok", "ok", "declined", "expected — not marked RunsOnRustClr");
                }
                continue;
            }

            if (!template.RunsOnRustClr)
            {
                // Not a failure, but the metadata is now wrong, and wrong
                // metadata is what the Deploy dialog reads.
                Report(template, TierOf(template), "ok", "ok", "ok", "runs, but RunsOnRustClr is false");
                continue;
            }

            var same = string.Equals(
                Normalise(onDotnet.StandardOutput),
                Normalise(onRustClr.StandardOutput),
                StringComparison.Ordinal);

            if (!same)
            {
                Report(template, TierOf(template), "ok", "ok", "ok", "OUTPUT DIFFERS");
                Console.WriteLine("    .NET:    " + Excerpt(onDotnet.StandardOutput));
                Console.WriteLine("    RustCLR: " + Excerpt(onRustClr.StandardOutput));
                failures++;
                continue;
            }

            Report(template, TierOf(template), "ok", "ok", "ok", "identical");
        }

        Console.WriteLine();
        if (!keepArtifacts)
        {
            try { Directory.Delete(root, recursive: true); } catch { /* best effort */ }
        }
        else
        {
            Console.WriteLine($"Artifacts kept in {root}");
        }

        if (failures > 0)
        {
            Console.WriteLine($"{failures} template(s) failed.");
            return 1;
        }
        Console.WriteLine(
            $"All {templates.Count} template(s) built; "
            + $"{templates.Count - skipped} ran identically on both runtimes.");
        return 0;
    }

    private static string TierOf(ProjectTemplate template) => template.MinimumTier switch
    {
        BclTier.Minimal => "minimal",
        BclTier.Full => "full",
        _ => "-",
    };

    private static void Report(
        ProjectTemplate template, string tier, string build, string dotnet, string rustclr, string note)
        => Console.WriteLine($"{template.Id,-22} {tier,-8} {build,-7} {dotnet,-7} {rustclr,-8} {note}");

    /// <summary>Line endings differ by runtime host; the bytes otherwise must not.</summary>
    private static string Normalise(string text) =>
        text.Replace("\r\n", "\n").TrimEnd('\n');

    private static string FirstError(string output)
    {
        foreach (var line in output.Split('\n'))
        {
            // `[rustnet]` lines are informational and go to stderr; reporting
            // one as "the error" hides the actual failure underneath it.
            if (line.TrimStart().StartsWith("[rustnet]", StringComparison.Ordinal)) continue;

            if (line.Contains("error", StringComparison.OrdinalIgnoreCase)
                || line.Contains("no implementation", StringComparison.OrdinalIgnoreCase)
                || line.Contains("Unhandled exception", StringComparison.OrdinalIgnoreCase))
            {
                return line.Trim();
            }
        }
        var first = output.Split('\n').FirstOrDefault(l => l.Trim().Length > 0);
        return first?.Trim() ?? "(no output)";
    }

    private static string Excerpt(string text)
    {
        var flat = Normalise(text).Replace("\n", " | ");
        return flat.Length <= 90 ? flat : flat[..90] + "…";
    }
}
