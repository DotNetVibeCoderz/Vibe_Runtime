using System;
using System.Collections.Generic;

namespace Conformance;

// MathF, the float overloads of Math, and typed arrays from generic
// collections -- gaps met by Buitenzorg.Three (a 3D scene graph written with
// floats). Expected values were produced by dotnet.
static partial class Program
{
    static void FloatMathChecks()
    {
        float a = 2.25f, b = -1.5f;
        CheckStr("MathF basics",
            $"{MathF.Sqrt(a)}|{MathF.Abs(b)}|{MathF.Min(a, b)}|{MathF.Max(a, b)}|{MathF.Floor(b)}|{MathF.Ceiling(b)}|{MathF.Round(2.5f)}|{MathF.Sign(b)}|{MathF.Pow(2f, 10f)}",
            "1.5|1.5|-1.5|2.25|-2|-1|2|-1|1024");
        CheckStr("MathF trigonometry",
            $"{MathF.Round(MathF.Sin(MathF.PI / 2) * 1000)}|{MathF.Round(MathF.Cos(0f) * 1000)}|{MathF.Round(MathF.Tan(MathF.PI / 4) * 1000)}|{MathF.Round(MathF.Atan2(1f, 1f) * 1000)}|{MathF.Round(MathF.Asin(1f) * 1000)}|{MathF.Round(MathF.Acos(0f) * 1000)}|{MathF.Round(MathF.Atan(1f) * 1000)}",
            "1000|1000|1000|785|1571|1571|785");
        CheckStr("MathF exp / log",
            $"{MathF.Round(MathF.Exp(1f) * 1000)}|{MathF.Log(1f)}|{MathF.Log10(1000f)}|{MathF.Log2(8f)}|{MathF.Truncate(-2.7f)}|{MathF.Cbrt(27f)}",
            "2718|0|3|3|-2|3");
        CheckStr("Math float overloads",
            $"{Math.Abs(b)}|{Math.Min(a, b)}|{Math.Max(a, b)}|{Math.Sign(b)}|{Math.Clamp(5f, 0f, 1f)}|{Math.Clamp(-0.5f, 0f, 1f)}",
            "1.5|-1.5|2.25|-1|1|0");

        var floats = new List<float> { 1.5f, 2.5f };
        float[] fa = floats.ToArray();
        CheckStr("List<float>.ToArray is a float[]", $"{fa.GetType().Name}|{((object)fa is float[])}|{fa[1]}|{fa.Length}", "Single[]|True|2.5|2");
        var ints = new List<int> { 7, 8, 9 };
        CheckStr("List<int>.ToArray is an int[]", $"{ints.ToArray().GetType().Name}|{ints.ToArray()[2]}", "Int32[]|9");
        var strs = new List<string> { "x" };
        CheckStr("List<string>.ToArray is a string[]", strs.ToArray().GetType().Name, "String[]");
        CheckStr("GetType of a vector and typeof(T[]) agree",
            $"{new float[2].GetType().Name}|{typeof(float[]).Name}|{new float[1].GetType() == typeof(float[])}|{new string[0].GetType().FullName}|{typeof(int[]).GetElementType()!.Name}",
            "Single[]|Single[]|True|System.String[]|Int32");
    }
}
