// Program half of the cross-assembly fixture. Prints a FAIL line per mismatch
// and `checks=N failures=M` at the end, like the conformance fixture.
using CrossLib;

var checks = 0;
var failures = 0;
void Check(string label, string got, string want)
{
    checks++;
    if (got == want) return;
    failures++;
    Console.WriteLine($"FAIL {label} (got '{got}' want '{want}')");
}

Check("interface implemented in another assembly", Pipeline.CountAsync(new Letters()).GetAwaiter().GetResult().ToString(), "2");

var registry = new Registry();
registry.Put(new Pt(1, 2));
registry.Put(7);
registry.Put(new Pt(3, 4));
Check("generic construction over a caller's type argument", $"{registry.Get<Pt>(1).Y}|{registry.Get<int>(0)}", "4|7");

Check("typeof(T) in an async method of a library generic", new Store<Pt>().DescribeAsync().GetAwaiter().GetResult(), "Pt:Pt { X = 0, Y = 0 }");

Check("a program's dictionary subclass seen by library code", Inspect.Kinds(new Bag { ["k"] = 1 }), "TrueTrue");

// Unoptimised builds (this project's) make every async state machine a class.
// Resuming one used to pass a pointer where the object belonged, so every
// field read after the first suspended `await` came back null.
var handshake = new Handshake();
var loader = new Loader();
var loading = loader.LoadAsync(handshake);
handshake.Complete(7);
Check("a class state machine resumes after a suspended await", $"{loading.IsCompleted}|{loader.Opened?.Name}|{loader.Steps}", "True|board#7|open,resumed");

// Interface dispatch from library code onto an override in this assembly.
// Rustzor's renderer calls `IComponent.SetParametersAsync` exactly this way.
var part = new Part();
var described = PartHost.Drive(part);
Check("an override is found through a library interface", $"{described}|{part.Log}", "part|part:x;base:x;");

// A library grid searching a program's record type: nested closures over a
// type argument from the other assembly. Rustzor.UI's DataGrid does this.
// A first instantiation searched before the second: generic code shared by
// two type arguments must not keep the first one's closure types.
var rows = new List<Row> { new("Hand grinder", 3), new("Kettle", 1) };
var rowGrid = new Grid<Row> { Items = rows, Columns = new() { new("Name", r => r.Name), new("Units", r => r.Units) } };
var firstSearch = rowGrid.Search("kettle").Count;
var sales = new List<Sale> { new("SO-1", "Hand grinder", 2, 525.0), new("SO-2", "Tumbler", 1, 145.0), new("SO-3", "Hand grinder", 1, 525.0) };
var grid = new Grid<Sale> { Items = sales, Columns = new() { new("Id", s => s.Id), new("Product", s => s.Product), new("Qty", s => s.Qty), new("Total", s => s.Total) { Format = "N0" } } };
Check("a library generic searches a program's records", $"{firstSearch}|{grid.Search("grinder").Count}|{grid.Search("1,050").Count}|{grid.Search("SO-2")[0].Product}", "1|2|1|Tumbler");

var handlers = new Handlers();
var heard = "";
handlers.On("raw", args => heard += $"raw:{args.Length};");
handlers.On<string>("one", s => heard += $"one:{s.Length};");
handlers.On<string, int>("two", (s, n) => heard += $"two:{s}{n};");
handlers.Fire("raw", 1, 2);
handlers.Fire("one", "abc");
handlers.Fire("two", "x", 5);
Check("overloads that differ only in generic arity", heard, "raw:2;one:3;two:x5;");
Console.WriteLine($"checks={checks} failures={failures}");

sealed record Row(string Name, int Units);

sealed record Sale(string Id, string Product, int Qty, double Price)
{
    public double Total => Qty * Price;
}

sealed class Part : PartBase
{
    public override Task SetAsync(PartValues values) { Log += "part:" + values.Value + ";"; return base.SetAsync(values); }
    public override string Describe() => "part";
}

sealed class Loader
{
    public Handshake Opened;
    public string Steps = "";

    public async Task LoadAsync(Handshake handshake)
    {
        Steps = "open";
        Opened = await Handshake.OpenAsync("board", handshake);
        Steps += ",resumed";
    }
}

readonly record struct Pt(double X, double Y);

sealed class Bag : Dictionary<string, object> { }

sealed class Letters : IEmbedder
{
    public Task<List<float[]>> EmbedAsync(IReadOnlyList<string> texts) =>
        Task.FromResult(texts.Select(t => new float[] { t.Length }).ToList());
}
