using System;
using System.Collections.Generic;
using System.Linq;

namespace Conformance;

// Enums as keys and as boxed values: the shapes a dashboard's lookup tables
// take. Expected values were produced by dotnet.
static partial class Program
{
    enum Fuel { Coal, Gas, Hydro, Geothermal, Solar, Wind }
    enum Tiny : byte { Low = 1, High = 200 }
    enum Signed : short { Down = -300, Up = 300 }
    enum Far64 : long { Away = 5_000_000_000 }

    // Six or more elements: Roslyn initialises these from an image blob.
    static readonly Fuel[] Fuels = { Fuel.Coal, Fuel.Gas, Fuel.Hydro, Fuel.Geothermal, Fuel.Solar, Fuel.Wind };
    static readonly Tiny[] Tinies = { Tiny.High, Tiny.Low, Tiny.High, Tiny.Low, Tiny.High, Tiny.Low, Tiny.High, Tiny.Low };
    static readonly Signed[] Signeds = { Signed.Down, Signed.Up, Signed.Down, Signed.Up, Signed.Down, Signed.Up };
    static readonly Far64[] Fars = { Far64.Away, Far64.Away, Far64.Away, Far64.Away, Far64.Away, Far64.Away };
    static Dictionary<Fuel, double> Mix { get; } = Fuels.ToDictionary(f => f, f => (double)f * 10);

    static void EnumChecks()
    {
        CheckStr("enum arrays from an image blob",
            $"{string.Join(",", Fuels)}|{string.Join(",", Tinies.Take(2))}|{(int)Tinies[0]}|{string.Join(",", Signeds.Take(2))}|{(short)Signeds[0]}|{(long)Fars[5]}",
            "Coal,Gas,Hydro,Geothermal,Solar,Wind|High,Low|200|Down,Up|-300|5000000000");
        CheckStr("enum-keyed dictionary built from a static array",
            $"{Mix.Count}|{Mix[Fuel.Hydro]}|{Mix.ContainsKey(Fuel.Wind)}|{Mix.Where(kv => kv.Key >= Fuel.Solar).Sum(kv => kv.Value)}",
            "6|20|True|90");
        object a = Fuel.Hydro, b = Fuel.Hydro, c = Fuel.Gas;
        CheckStr("boxed enums compare and hash by value",
            $"{a.Equals(b)}|{a.Equals(c)}|{a.Equals(2)}|{Fuel.Hydro.Equals(Fuel.Hydro)}|{Fuel.Hydro.GetHashCode()}|{a.GetHashCode()}|{Fuel.Hydro.CompareTo(Fuel.Gas)}|{new HashSet<object> { Fuel.Hydro, Fuel.Gas, Fuel.Hydro }.Count}",
            "True|False|False|True|2|2|1|2");
    }
}
