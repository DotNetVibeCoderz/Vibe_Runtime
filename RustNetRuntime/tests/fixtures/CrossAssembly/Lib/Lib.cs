// Library half of the cross-assembly fixture: generic code and interfaces that
// a program in another assembly instantiates and implements. Each shape here
// once failed only when the two halves lived in different assemblies.
namespace CrossLib;

public interface IEmbedder
{
    Task<List<float[]>> EmbedAsync(IReadOnlyList<string> texts);
}

public static class Pipeline
{
    public static async Task<int> CountAsync(IEmbedder embedder) => (await embedder.EmbedAsync(new[] { "a", "bb" })).Count;
}

public sealed class Slots<T>
{
    private T[] _items = new T[2];
    public int Count { get; private set; }
    public void Add(T item)
    {
        if (Count == _items.Length) Array.Resize(ref _items, Count * 2);
        _items[Count++] = item;
    }
    public T At(int i) => _items[i];
}

public sealed class Registry
{
    private readonly Dictionary<Type, object> _slots = new();
    private Slots<T> Slots<T>()
    {
        if (_slots.TryGetValue(typeof(T), out var s)) return (Slots<T>)s;
        var created = new Slots<T>();
        _slots[typeof(T)] = created;
        return created;
    }
    public void Put<T>(T item) => Slots<T>().Add(item);
    public T Get<T>(int i) => Slots<T>().At(i);
}

public sealed class Store<T> where T : new()
{
    public async Task<string> DescribeAsync()
    {
        await Task.CompletedTask;
        return typeof(T).Name + ":" + new T();
    }
}

public static class Inspect
{
    public static string Kinds(object value) =>
        $"{value is System.Collections.IEnumerable}{value is System.Collections.IDictionary}";
}

/// <summary>
/// An async API that really suspends: its task completes only when the caller
/// calls <see cref="Complete"/>, the way a browser callback completes a fetch.
/// </summary>
public sealed class Handshake
{
    private readonly TaskCompletionSource<int> _pending = new();

    public static async Task<Handshake> OpenAsync(string name, Handshake handshake)
    {
        var handle = await handshake._pending.Task;
        handshake.Name = $"{name}#{handle}";
        return handshake;
    }

    public string Name { get; private set; } = "";
    public void Complete(int handle) => _pending.SetResult(handle);
}

/// <summary>
/// The component shape: an interface the library calls through, implemented
/// by a library base class with virtual methods that a program overrides.
/// </summary>
public readonly struct PartValues
{
    public PartValues(string value) => Value = value;
    public string Value { get; }
}

public interface IPart
{
    Task SetAsync(PartValues values);
    string Describe();
}

public abstract class PartBase : IPart
{
    public string Log = "";
    public virtual Task SetAsync(PartValues values) { Log += "base:" + values.Value + ";"; return Task.CompletedTask; }
    public virtual string Describe() => "base";
}

public static class PartHost
{
    public static string Drive(IPart part)
    {
        part.SetAsync(new PartValues("x")).GetAwaiter().GetResult();
        return part.Describe();
    }
}

/// <summary>A column of a grid over a caller's row type, as Rustzor.UI's DataGrid has.</summary>
public sealed class Column<TItem>
{
    public Column(string title, Func<TItem, object> value) { Title = title; Value = value; }
    public string Title { get; }
    public Func<TItem, object> Value { get; }
    public string Format { get; init; }

    public string Text(TItem item)
    {
        var v = Value(item);
        return v switch
        {
            null => "",
            double d when Format is not null => d.ToString(Format),
            int i when Format is not null => i.ToString(Format),
            DateTime dt => dt.ToString(Format ?? "yyyy-MM-dd"),
            _ => v.ToString() ?? "",
        };
    }
}

/// <summary>Search across columns: a closure over a closure, generic over the caller's type.</summary>
public sealed class Grid<TItem>
{
    public IReadOnlyList<TItem> Items { get; set; } = Array.Empty<TItem>();
    public List<Column<TItem>> Columns { get; set; } = new();

    public List<TItem> Search(string query) =>
        Items.Where(item => Columns.Any(c => c.Text(item).Contains(query, StringComparison.OrdinalIgnoreCase))).ToList();
}

/// <summary>
/// Overloads that differ only in generic arity, as SignalR's <c>On</c> has:
/// the generic one must not bind to the raw one.
/// </summary>
public sealed class Handlers
{
    private readonly Dictionary<string, Action<object?[]>> _raw = new();
    public void On(string name, Action<object?[]> handler) => _raw[name] = handler;
    public void On<T1>(string name, Action<T1> handler) => On(name, args => handler((T1)args[0]!));
    public void On<T1, T2>(string name, Action<T1, T2> handler) => On(name, args => handler((T1)args[0]!, (T2)args[1]!));
    public void Fire(string name, params object?[] args) => _raw[name](args);
}
