using System;

namespace Conformance;

// An event whose first handler unsubscribes the second: the invocation in progress
// still runs the second (it works on a snapshot), whose target is now reachable only
// from that invocation. Rustzor hit this when a page left on LocationChanged: the
// router's handler disposed the page, and the page's own handler then ran on a
// collected object. Called under constant collection by crates/rustclr-bcl/tests/gc_natives.rs.
// Expected values were produced by dotnet.
static partial class Program
{
    static event Action? GcChanged;
    static int GcEventTotal;

    sealed class GcLeavingPage
    {
        private readonly string _name;
        public GcLeavingPage(string name) => _name = name;
        public void OnChanged() => GcEventTotal += _name.Length;
    }

    static void GcRouterHandler()
    {
        // The router re-renders and the page unsubscribes: only this handler remains.
        GcChanged = GcRouterHandler;
        var garbage = 0;
        for (var k = 0; k < 24; k++) garbage += new string('x', 64 + k).Length;
        if (garbage < 0) GcEventTotal = -1;
    }

    static int GcEventUnsubscribe(int n)
    {
        GcEventTotal = 0;
        for (var i = 0; i < n; i++)
        {
            GcChanged = GcRouterHandler;
            GcChanged += new GcLeavingPage("page" + i).OnChanged;
            GcChanged?.Invoke();
        }
        GcChanged = null;
        return GcEventTotal;
    }

    static void GcEventChecks() =>
        CheckEq("a handler unsubscribed by an earlier one still runs, on a live target", GcEventUnsubscribe(300), 1990);
}
