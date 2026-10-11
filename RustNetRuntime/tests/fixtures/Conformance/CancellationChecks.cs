using System;
using System.Threading;

namespace Conformance;

// Cancellation as library code uses it: a source, its token, callbacks, and
// the exception. Expected values were produced by dotnet.
static partial class Program
{
    static void CancellationChecks()
    {
        var none = CancellationToken.None;
        var cts = new CancellationTokenSource();
        var token = cts.Token;
        var log = "";
        token.Register(() => log += "a;");
        token.Register(state => log += $"{state};", "b");
        var before = $"{none.CanBeCanceled}|{none.IsCancellationRequested}|{token.CanBeCanceled}|{token.IsCancellationRequested}";
        cts.Cancel();
        cts.Cancel();
        token.Register(() => log += "late;");
        var threw = "";
        try { token.ThrowIfCancellationRequested(); }
        catch (OperationCanceledException e) { threw = e.GetType().Name; }
        CheckStr("cancellation tokens, callbacks and the exception",
            $"{before}|{token.IsCancellationRequested}|{cts.IsCancellationRequested}|{log}|{threw}",
            "False|False|True|False|True|True|b;a;late;|OperationCanceledException");
    }
}
