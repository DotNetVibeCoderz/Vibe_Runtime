using System;

namespace Conformance;

// `this` of a value type's own method is a managed pointer, also when the call
// came through `constrained.` or a box (ECMA-335 I.8.9.7, III.2.1). RustCLR
// used to hand the method a by-value copy, so `A + "x"` inside a struct's
// ToString (`ldarg.0; ldflda; call Int32::ToString`) threw
// NullReferenceException. Expected values were produced by dotnet.
static partial class Program
{
    interface IBump { void Bump(); int Value { get; } }

    struct Counter : IBump
    {
        public int A, B;
        public float F;
        public void Bump() => A++;
        public int Value => A;
        public override string ToString() => A + " a, " + B + " b, " + F + " f";
        public override int GetHashCode() => A.GetHashCode() ^ B;
    }

    static void BumpGeneric<T>(ref T t) where T : IBump => t.Bump();
    static string Describe<T>(T t) => t!.ToString()!;

    static void StructReceiverChecks()
    {
        var c = new Counter { A = 3, B = 4, F = 1.5f };
        CheckStr("struct ToString reads fields through this", c.ToString(), "3 a, 4 b, 1.5 f");
        CheckStr("struct ToString through string concatenation", "c=" + c, "c=3 a, 4 b, 1.5 f");
        object boxed = c;
        CheckStr("struct ToString on a boxed instance", boxed.ToString()!, "3 a, 4 b, 1.5 f");
        CheckStr("struct ToString through a generic (constrained.)", Describe(c), "3 a, 4 b, 1.5 f");
        CheckEq("struct GetHashCode override through a box", boxed.GetHashCode(), 3 ^ 4);

        // a mutating interface call on a box changes the box, not a copy
        IBump ib = c;
        ib.Bump(); ib.Bump();
        CheckEq("interface call on a boxed struct mutates the box", ib.Value, 5);
        CheckEq("... and leaves the original local alone", c.A, 3);

        // constrained. on a ref T mutates the caller's variable
        BumpGeneric(ref c);
        CheckEq("constrained. call through ref T mutates the variable", c.A, 4);
    }
}
