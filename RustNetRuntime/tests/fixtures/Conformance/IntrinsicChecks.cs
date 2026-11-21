using System;
using System.Threading.Channels;

namespace Conformance;

// Milestone 8.4: the methods RustCLR substitutes a Rust native for
// (crates/rustclr-bcl/src/intrinsics.rs). `ChannelRingChecksum` is also run by
// crates/rustclr-bcl/tests/intrinsics.rs with intrinsics on and off, and the
// two must agree — so it reaches every shape the natives take and every shape
// they decline: a ring that grows, wraps, drops from either end, and holds
// ints, longs, doubles, bools, strings and structs.
static partial class Program
{
    static void IntrinsicChecks()
    {
        Check("a channel's ring keeps every item in order across growth and wrap", ChannelRingChecksum(64) == -2_829_831_623_697_927_011L);
    }

    struct RingPair
    {
        public int A;
        public string B;
    }

    static long RingMix(long h, long v) => unchecked((h ^ v) * 1_099_511_628_211L);

    static long ChannelRingChecksum(int n)
    {
        long h = 1_469_598_103_934_665_603L;

        // Unbounded: the ring grows from 4 while it is written, then drains.
        var ints = Channel.CreateUnbounded<int>();
        for (int i = 0; i < n; i++) ints.Writer.TryWrite(i * 3);
        while (ints.Reader.TryRead(out int x)) h = RingMix(h, x);

        // Interleaved, so the head walks round the ring many times.
        var longs = Channel.CreateUnbounded<long>();
        for (int i = 0; i < n; i++)
        {
            longs.Writer.TryWrite(i * 1_000_000_007L);
            longs.Writer.TryWrite(-i);
            if (longs.Reader.TryRead(out long y)) h = RingMix(h, y);
        }
        while (longs.Reader.TryRead(out long y)) h = RingMix(h, y);

        var doubles = Channel.CreateUnbounded<double>();
        for (int i = 0; i < n; i++) doubles.Writer.TryWrite(i / 4.0);
        while (doubles.Reader.TryRead(out double d)) h = RingMix(h, (long)(d * 8));

        var bools = Channel.CreateUnbounded<bool>();
        for (int i = 0; i < n; i++) bools.Writer.TryWrite(i % 3 == 0);
        while (bools.Reader.TryRead(out bool b)) h = RingMix(h, b ? 7 : 11);

        var strings = Channel.CreateUnbounded<string>();
        for (int i = 0; i < n; i++)
        {
            strings.Writer.TryWrite("s" + i);
            if (i % 2 == 0 && strings.Reader.TryRead(out string s)) h = RingMix(h, s.Length * 31 + s[s.Length - 1]);
        }
        while (strings.Reader.TryRead(out string s)) h = RingMix(h, s.Length * 31 + s[s.Length - 1]);

        // A ring of structs: the natives decline it, and the IL runs.
        var pairs = Channel.CreateUnbounded<RingPair>();
        for (int i = 0; i < n; i++) pairs.Writer.TryWrite(new RingPair { A = i, B = "p" + i });
        while (pairs.Reader.TryRead(out RingPair p)) h = RingMix(h, p.A * 17 + p.B.Length);

        // Bounded, each way of being full.
        foreach (var mode in new[] { BoundedChannelFullMode.DropOldest, BoundedChannelFullMode.DropNewest, BoundedChannelFullMode.DropWrite, BoundedChannelFullMode.Wait })
        {
            var bounded = Channel.CreateBounded<int>(new BoundedChannelOptions(5) { FullMode = mode });
            int accepted = 0;
            for (int i = 0; i < n; i++)
            {
                if (bounded.Writer.TryWrite(i)) accepted++;
                if (i % 7 == 0 && bounded.Reader.TryRead(out int z)) h = RingMix(h, z + 1000);
            }
            h = RingMix(h, accepted);
            while (bounded.Reader.TryRead(out int z)) h = RingMix(h, z);
        }
        return h;
    }
}
