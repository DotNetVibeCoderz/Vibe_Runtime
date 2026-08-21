namespace CodeGen.Models;

/// <summary>What kind of application a template produces.</summary>
public enum TemplateCategory
{
    Console,
    Web,
    Desktop,
    Mobile,
    IoT,
    Library,
}

/// <summary>The field a template comes from, used to group the picker.</summary>
public enum TemplateDomain
{
    Business,
    Science,
    Education,
    Games,
    Runtime,
}

/// <summary>One file a template writes into the new project.</summary>
public sealed record TemplateFile(string RelativePath, string Contents);

/// <summary>A project template.</summary>
public sealed record ProjectTemplate
{
    public required string Id { get; init; }
    public required string Name { get; init; }
    public required string Summary { get; init; }
    public required TemplateCategory Category { get; init; }
    public required TemplateDomain Domain { get; init; }
    public required IReadOnlyList<TemplateFile> Files { get; init; }

    /// <summary>Command shown in the UI for how to run the result.</summary>
    public string RunHint { get; init; } = "dotnet run";

    /// <summary>True when the output can execute on RustCLR as well as .NET.</summary>
    public bool RunsOnRustClr { get; init; } = true;

    /// <summary>
    /// The smallest board tier this template's code will run on.
    ///
    /// <see cref="BclTier.Minimal"/> means it stays inside `Console`, `String`,
    /// `Math` and arrays — no generic collections, LINQ or reflection — so it
    /// runs on a board that cannot hold all of RustBCL. That restriction is the
    /// difference between a template that deploys to a Pico and one that only
    /// deploys to an ESP32, which is why it is recorded rather than implied.
    ///
    /// <see cref="BclTier.None"/> would mean "desktop only"; nothing uses it,
    /// because a template that cannot run anywhere is not a template.
    /// </summary>
    public BclTier MinimumTier { get; init; } = BclTier.Full;

    /// <summary>Whether this template is meant to be flashed to a board.</summary>
    public bool IsEmbedded => Category == TemplateCategory.IoT;

    public string TierLabel => MinimumTier switch
    {
        BclTier.Minimal => "any board",
        BclTier.Full => "full-RustBCL boards",
        _ => "desktop only",
    };

    public string CategoryLabel => Category switch
    {
        TemplateCategory.Console => "Console",
        TemplateCategory.Web => "Web API",
        TemplateCategory.Desktop => "Desktop",
        TemplateCategory.Mobile => "Mobile",
        TemplateCategory.IoT => "IoT",
        TemplateCategory.Library => "Library",
        _ => Category.ToString(),
    };

    public string DomainLabel => Domain.ToString();
}
