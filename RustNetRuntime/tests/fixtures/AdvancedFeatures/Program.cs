using System;
using System.Collections.Generic;
using System.Linq;
using System.Runtime.InteropServices;
using System.Threading;
using System.Threading.Tasks;

namespace AdvancedFeatures;

// ── C# 12: primary constructors ─────────────────────────────────────────────
public class Sensor(string id, double scale)
{
    public string Id => id;
    public double Calibrate(int raw) => raw * scale;
}

public readonly struct Reading(double value)
{
    public double Value => value;
}

// ── C# 9: records ───────────────────────────────────────────────────────────
public record Measurement(string Sensor, double Value);

// ── C# 14: extension members ────────────────────────────────────────────────
public static class ArrayExtensions
{
    extension(int[] values)
    {
        public int Second => values[1];
        public int SumAll()
        {
            int total = 0;
            for (int i = 0; i < values.Length; i++) total += values[i];
            return total;
        }
    }
}

// ── Dispose ─────────────────────────────────────────────────────────────────
public sealed class Resource : IDisposable
{
    public static int Closed;
    public void Dispose() => Closed++;
}

public sealed class AsyncResource : IAsyncDisposable
{
    public static int Closed;
    public ValueTask DisposeAsync()
    {
        Closed++;
        return ValueTask.CompletedTask;
    }
}

/// <summary>
/// One advanced feature per run, so a failure in one does not hide the others.
///
/// Each probe prints `PASS &lt;name&gt;` and its computed value. The harness runs
/// every probe on both runtimes and compares — a feature "works" only when the
/// two agree.
/// </summary>
public static class Program
{
    public static int Main(string[] args)
    {
        string probe = args.Length > 0 ? args[0] : "list";

        switch (probe)
        {
            case "async-await": return AsyncAwait();
            case "tpl": return Tpl();
            case "threading": return Threading();
            case "gc": return Gc();
            case "dispose": return Dispose();
            case "dispose-async": return DisposeAsync();
            case "span": return Spans();
            case "primary-ctor": return PrimaryConstructors();
            case "collection-expr": return CollectionExpressions();
            case "collection-expr-spread": return CollectionExpressionsWithSpread();
            case "collection-expr-span": return CollectionExpressionsToSpan();
            case "extension-members": return ExtensionMembers();
            case "pinvoke": return PInvoke();
            case "marshalling": return Marshalling();
            case "unsafe-fixed": return UnsafeFixed();
            case "unsafe-stackalloc": return UnsafeStackalloc();
            case "linq": return Linq();
            case "pattern-matching": return PatternMatching();
            case "records": return Records();
            case "generated": return Generated();
            case "interceptor": return Interceptor();
            default:
                Console.WriteLine(
                    "probes: async-await tpl threading gc dispose dispose-async span "
                    + "primary-ctor collection-expr collection-expr-span extension-members "
                    + "pinvoke marshalling unsafe-fixed unsafe-stackalloc linq "
                    + "pattern-matching records generated interceptor");
                return 2;
        }
    }

    private static int Pass(string name, object value)
    {
        Console.WriteLine("PASS " + name + " " + value);
        return 0;
    }

    // ── Asynchronous and parallel ───────────────────────────────────────────

    private static int AsyncAwait()
    {
        int result = ComputeAsync(20).GetAwaiter().GetResult();
        return Pass("async-await", result);
    }

    private static async Task<int> ComputeAsync(int seed)
    {
        await Task.Yield();
        int doubled = await Task.FromResult(seed * 2);
        await Task.Delay(1);
        return doubled + 2;
    }

    private static int Tpl()
    {
        var first = Task.Run(() => 20);
        var second = Task.Run(() => 22);
        Task.WaitAll(first, second);

        int total = first.Result + second.Result;

        int parallelTotal = 0;
        Parallel.For(0, 10, i => Interlocked.Add(ref parallelTotal, i));

        return Pass("tpl", total + "/" + parallelTotal);
    }

    private static int Threading()
    {
        int counter = 0;
        object gate = new object();

        var thread = new Thread(() =>
        {
            for (int i = 0; i < 1000; i++)
            {
                lock (gate) counter++;
            }
        });
        thread.Start();
        thread.Join();

        Thread.Sleep(1);
        return Pass("threading", counter);
    }

    // ── Memory and resources ────────────────────────────────────────────────

    private static int Gc()
    {
        for (int i = 0; i < 20000; i++)
        {
            object garbage = new object();
            if (garbage == null) return 1;
        }
        GC.Collect();
        long after = GC.GetTotalMemory(true);
        return Pass("gc", after > 0 ? "collected" : "empty");
    }

    private static int Dispose()
    {
        Resource.Closed = 0;
        using (var one = new Resource())
        {
            if (one == null) return 1;
        }
        using var two = new Resource();
        // The declaration form disposes at method exit, so only the block form
        // has run by now.
        return Pass("dispose", Resource.Closed);
    }

    private static int DisposeAsync()
    {
        AsyncResource.Closed = 0;
        RunAsyncDispose().GetAwaiter().GetResult();
        return Pass("dispose-async", AsyncResource.Closed);
    }

    private static async Task RunAsyncDispose()
    {
        await using (var resource = new AsyncResource())
        {
            await Task.Yield();
        }
    }

    private static int Spans()
    {
        Span<int> numbers = stackalloc int[4];
        for (int i = 0; i < numbers.Length; i++) numbers[i] = i * i;

        Span<int> tail = numbers.Slice(2);
        int total = 0;
        for (int i = 0; i < tail.Length; i++) total += tail[i];

        int[] backing = new int[] { 1, 2, 3, 4 };
        Memory<int> memory = backing.AsMemory(1, 2);
        Span<int> fromMemory = memory.Span;

        return Pass("span", total + "/" + fromMemory[0] + fromMemory[1]);
    }

    // ── Modern language features ────────────────────────────────────────────

    private static int PrimaryConstructors()
    {
        var sensor = new Sensor("temp-1", 0.5);
        var reading = new Reading(21.5);
        return Pass("primary-ctor", sensor.Id + "/" + sensor.Calibrate(84) + "/" + reading.Value);
    }

    /// <summary>A collection expression targeting an array.</summary>
    private static int CollectionExpressions()
    {
        int[] numbers = [1, 2, 3];

        int total = 0;
        for (int i = 0; i < numbers.Length; i++) total += numbers[i];

        return Pass("collection-expr", total + "/" + numbers.Length);
    }

    /// <summary>
    /// The spread form, which the compiler lowers through `Span&lt;T&gt;` — so
    /// it asks a different question than the plain literal above.
    /// </summary>
    private static int CollectionExpressionsWithSpread()
    {
        int[] numbers = [1, 2, 3];
        int[] spread = [..numbers, 4];

        int total = 0;
        for (int i = 0; i < spread.Length; i++) total += spread[i];

        return Pass("collection-expr-spread", total);
    }

    /// <summary>
    /// The same syntax targeting a span, which needs `ReadOnlySpan&lt;T&gt;` —
    /// a generic type, and therefore a separate question.
    /// </summary>
    private static int CollectionExpressionsToSpan()
    {
        ReadOnlySpan<char> letters = ['a', 'b'];
        return Pass("collection-expr-span", letters.Length);
    }

    private static int ExtensionMembers()
    {
        int[] values = [10, 20, 30];
        return Pass("extension-members", values.Second + "/" + values.SumAll());
    }

    // ── Interop ─────────────────────────────────────────────────────────────

    [DllImport("kernel32.dll")]
    private static extern uint GetCurrentProcessId();

    [DllImport("libc", EntryPoint = "getpid")]
    private static extern int GetPid();

    private static int PInvoke()
    {
        // Both declarations exist; only the one for this platform is called.
        uint id = OperatingSystem.IsWindows() ? GetCurrentProcessId() : (uint)GetPid();
        return Pass("pinvoke", id > 0 ? "got-pid" : "no-pid");
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct Point
    {
        public int X;
        public int Y;
    }

    private static int Marshalling()
    {
        int size = Marshal.SizeOf<Point>();
        IntPtr buffer = Marshal.AllocHGlobal(size);
        try
        {
            Marshal.StructureToPtr(new Point { X = 3, Y = 4 }, buffer, false);
            Point roundTripped = Marshal.PtrToStructure<Point>(buffer);
            return Pass("marshalling", size + "/" + roundTripped.X + roundTripped.Y);
        }
        finally
        {
            Marshal.FreeHGlobal(buffer);
        }
    }

    /// <summary>Pointer arithmetic over a pinned array.</summary>
    private static unsafe int UnsafeFixed()
    {
        int[] values = new int[] { 1, 2, 3, 4 };
        int total = 0;

        fixed (int* start = values)
        {
            for (int* p = start; p < start + values.Length; p++) total += *p;
        }

        return Pass("unsafe-fixed", total);
    }

    /// <summary>`stackalloc`, which compiles to the `localloc` instruction.</summary>
    private static unsafe int UnsafeStackalloc()
    {
        int* scratch = stackalloc int[3];
        scratch[0] = 7;
        scratch[1] = 8;
        scratch[2] = 9;

        return Pass("unsafe-stackalloc", scratch[0] + scratch[2]);
    }

    // ── High-level abstractions ─────────────────────────────────────────────

    private static int Linq()
    {
        int[] numbers = [1, 2, 3, 4, 5, 6];
        var evens = numbers.Where(n => n % 2 == 0).Select(n => n * 10).ToArray();
        int total = evens.Sum();

        var grouped = numbers.GroupBy(n => n % 3).OrderBy(g => g.Key).ToList();

        return Pass("linq", total + "/" + grouped.Count);
    }

    private static int PatternMatching()
    {
        object value = 42;
        string described = value switch
        {
            int n when n > 100 => "big",
            int n and > 40 => "answer",
            int => "small",
            string s => s,
            null => "null",
            _ => "other",
        };

        var measurement = new Measurement("t1", 21.5);
        string byProperty = measurement switch
        {
            { Value: > 30 } => "hot",
            { Value: > 20 } => "warm",
            _ => "cold",
        };

        return Pass("pattern-matching", described + "/" + byProperty);
    }

    private static int Records()
    {
        var original = new Measurement("t1", 21.5);
        var copy = original with { Value = 22.5 };
        bool equal = original == new Measurement("t1", 21.5);
        string text = original.ToString();

        return Pass("records", copy.Value + "/" + equal + "/" + (text.Length > 0));
    }

    // ── Compile-time generation ─────────────────────────────────────────────

    private static int Generated()
    {
        // Emitted by the source generator in Generator/.
        return Pass("generated", GeneratedInfo.Describe() + "/" + GeneratedInfo.SensorCount);
    }

    private static int Interceptor()
    {
        // The call below is rewritten at compile time by the interceptor in
        // Generator/. Without interception it returns "original".
        return Pass("interceptor", Interceptable.Which());
    }
}

public static class Interceptable
{
    public static string Which() => "original";
}
