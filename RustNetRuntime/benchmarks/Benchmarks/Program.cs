namespace Benchmarks;

/// <summary>
/// One workload per run, so the harness measures wall clock from the outside
/// and neither runtime gets to hide startup cost inside a timer.
///
/// Everything here uses arrays and explicit loops: no LINQ, no generic
/// collections. That is deliberate — a benchmark that only runs on one of the
/// two runtimes measures nothing useful, and staying allocation-light keeps the
/// figures about the interpreter rather than the collection implementations.
/// </summary>
public static class Program
{
    public static int Main(string[] args)
    {
        string workload = args.Length > 0 ? args[0] : "all";
        int scale = args.Length > 1 ? int.Parse(args[1]) : 1;

        switch (workload)
        {
            // Does nothing: measures process start, which both figures include
            // and which dominates the short workloads on .NET.
            case "noop": return Report("noop", 0);
            case "fib": return Report("fib", Fib(27 + scale - 1));
            case "sieve": return Report("sieve", Sieve(1000000 * scale));
            case "strings": return Report("strings", Strings(20000 * scale));
            case "matrix": return Report("matrix", Matrix(120));
            case "sort": return Report("sort", Sort(200000 * scale));
            case "alloc": return Report("alloc", Allocate(300000 * scale));
            case "virtual": return Report("virtual", VirtualCalls(2000000 * scale));
            case "exceptions": return Report("exceptions", Exceptions(50000 * scale));
            case "fields": return Report("fields", FieldAccess(3000000 * scale));
            case "kernels": return Report("kernels", Kernels(60000 * scale));
            case "inlined": return Report("inlined", Inlined(400000 * scale));
            default:
                Console.WriteLine("workloads: noop fib sieve strings matrix sort alloc virtual exceptions fields kernels inlined");
                return 2;
        }
    }

    /// <summary>Prints a checksum so a runtime cannot "win" by skipping work.</summary>
    private static int Report(string name, long checksum)
    {
        Console.WriteLine(name + " " + checksum);
        return 0;
    }

    // ── Recursive call overhead ─────────────────────────────────────────────
    private static long Fib(int n) => n < 2 ? n : Fib(n - 1) + Fib(n - 2);

    // ── Integer kernels: leaf methods, no arrays, no calls ──────────────────
    //
    // This is the shape RustCLR's x86-64 backend compiles, so it is the
    // workload that shows what native code generation is worth. Every other
    // workload here uses arrays or calls and is interpreted on both settings —
    // which is itself worth measuring, since it shows the JIT costs nothing
    // when it declines.
    private static long Kernels(int iterations)
    {
        long total = 0;
        for (int i = 0; i < iterations; i++)
        {
            total += FibIterative(i % 60);
            total += Collatz(1 + (i % 500));
            total += Mix(i);
            total += Gcd(i, 360);
        }
        return total;
    }

    // ── Inlinable calls ─────────────────────────────────────────────────────
    //
    // The same integer arithmetic as `kernels`, but factored into small static
    // leaves instead of written out longhand. Without the inliner the backend
    // declines `Step` outright — it contains calls — and the whole workload is
    // interpreted. With it, the leaves are spliced in and `Step` compiles.
    //
    // The point of measuring this separately: `kernels` was written to suit the
    // backend's limits, and code in the wild is not.
    private static long Inlined(int iterations)
    {
        long total = 0;
        for (int i = 0; i < iterations; i++) { total += Step(i); }
        return total;
    }

    private static int Step(int i)
    {
        return Mix2(Scale(i, 3), Scale(i + 1, 5)) + Clamp16(i * i) - Scale(i, 2);
    }

    private static int Scale(int v, int by) { return v * by + (v >> 1); }

    private static int Clamp16(int v) { return v & 0xFFFF; }

    private static int Mix2(int a, int b) { return (a ^ b) + (a & b) * 2; }

    private static long FibIterative(int n)
    {
        long a = 0, b = 1;
        for (int i = 0; i < n; i++) { long t = a + b; a = b; b = t; }
        return a;
    }

    private static int Collatz(int n)
    {
        int steps = 0;
        while (n != 1)
        {
            if (n % 2 == 0) n = n / 2;
            else n = 3 * n + 1;
            steps++;
        }
        return steps;
    }

    private static long Mix(long x)
    {
        x ^= x >> 33;
        x *= 0x5bd1e995;
        x ^= x >> 29;
        return x;
    }

    private static int Gcd(int a, int b)
    {
        while (b != 0) { int t = b; b = a % b; a = t; }
        return a;
    }

    // ── Array writes and a tight loop ───────────────────────────────────────
    private static long Sieve(int limit)
    {
        bool[] composite = new bool[limit];
        long count = 0;
        for (int p = 2; p < limit; p++)
        {
            if (composite[p]) continue;
            count++;
            for (long multiple = (long)p * p; multiple < limit; multiple += p)
            {
                composite[(int)multiple] = true;
            }
        }
        return count;
    }

    // ── String allocation and UTF-16 handling ───────────────────────────────
    private static long Strings(int rounds)
    {
        long total = 0;
        for (int i = 0; i < rounds; i++)
        {
            string s = "item-" + i;
            total += s.Length;
            if (s.IndexOf("-") > 0) total++;
        }
        return total;
    }

    // ── Floating-point array maths ──────────────────────────────────────────
    private static long Matrix(int n)
    {
        double[] a = new double[n * n];
        double[] b = new double[n * n];
        double[] c = new double[n * n];

        for (int i = 0; i < n * n; i++)
        {
            a[i] = (i % 7) + 0.5;
            b[i] = (i % 11) + 0.25;
        }

        for (int row = 0; row < n; row++)
        {
            for (int col = 0; col < n; col++)
            {
                double sum = 0;
                for (int k = 0; k < n; k++)
                {
                    sum += a[row * n + k] * b[k * n + col];
                }
                c[row * n + col] = sum;
            }
        }

        double total = 0;
        for (int i = 0; i < n * n; i++) total += c[i];
        return (long)total;
    }

    // ── Comparison-heavy work with branches ─────────────────────────────────
    private static long Sort(int count)
    {
        int[] values = new int[count];
        // A deterministic pseudo-random fill: the same input on both runtimes.
        int seed = 12345;
        for (int i = 0; i < count; i++)
        {
            seed = seed * 1103515245 + 12345;
            values[i] = seed & 0x7FFFFFFF;
        }

        QuickSort(values, 0, count - 1);

        long checksum = 0;
        for (int i = 0; i < count; i += count / 16)
        {
            checksum += values[i] % 1000;
        }
        return checksum;
    }

    private static void QuickSort(int[] values, int low, int high)
    {
        while (low < high)
        {
            int pivot = values[(low + high) / 2];
            int i = low;
            int j = high;
            while (i <= j)
            {
                while (values[i] < pivot) i++;
                while (values[j] > pivot) j--;
                if (i > j) break;
                int swap = values[i];
                values[i] = values[j];
                values[j] = swap;
                i++;
                j--;
            }
            // Recurse into the smaller side, loop on the larger: bounded depth.
            if (j - low < high - i)
            {
                QuickSort(values, low, j);
                low = i;
            }
            else
            {
                QuickSort(values, i, high);
                high = j;
            }
        }
    }

    // ── Allocation and collection pressure ──────────────────────────────────
    private sealed class Node
    {
        public int Value;
        public Node Next;
    }

    private static long Allocate(int count)
    {
        long total = 0;
        Node head = null;
        for (int i = 0; i < count; i++)
        {
            Node node = new Node();
            node.Value = i;
            // Keep a short chain alive so the collector has real work, but let
            // the bulk become garbage.
            node.Next = (i % 64 == 0) ? head : null;
            if (i % 64 == 0) head = node;
            total += node.Value & 1;
        }
        return total;
    }

    // ── Virtual dispatch ────────────────────────────────────────────────────
    private abstract class Shape
    {
        public abstract int Sides();
    }

    private sealed class Triangle : Shape
    {
        public override int Sides() => 3;
    }

    private sealed class Square : Shape
    {
        public override int Sides() => 4;
    }

    private static long VirtualCalls(int count)
    {
        Shape[] shapes = new Shape[2];
        shapes[0] = new Triangle();
        shapes[1] = new Square();

        long total = 0;
        for (int i = 0; i < count; i++)
        {
            total += shapes[i & 1].Sides();
        }
        return total;
    }

    // ── Exception throw and catch ───────────────────────────────────────────
    private static long Exceptions(int count)
    {
        long caught = 0;
        for (int i = 0; i < count; i++)
        {
            try
            {
                if ((i & 3) == 0) throw new InvalidOperationException("planned");
                caught += 1;
            }
            catch (InvalidOperationException)
            {
                caught += 2;
            }
            finally
            {
                caught += 0;
            }
        }
        return caught;
    }

    // ── Instance field reads and writes ─────────────────────────────────────
    private sealed class Counter
    {
        public int Hits;
        public int Misses;
    }

    private static long FieldAccess(int count)
    {
        Counter counter = new Counter();
        for (int i = 0; i < count; i++)
        {
            if ((i & 7) == 0) counter.Hits++;
            else counter.Misses++;
        }
        return counter.Hits + counter.Misses;
    }
}
