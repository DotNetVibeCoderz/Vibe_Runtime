using System;
using System.Collections;
using System.Collections.Generic;

namespace Conformance;

// Framework collections made from a Type, the way serializers build them,
// must behave like ones made with `new`. Expected values were produced by dotnet.
static partial class Program
{
    static void ActivatorChecks()
    {
        var dict = (Dictionary<string, List<string>>)Activator.CreateInstance(typeof(Dictionary<string, List<string>>))!;
        dict["a"] = new List<string> { "x" };
        if (!dict.TryGetValue("b", out var list)) dict["b"] = list = new List<string>();
        list.Add("y");
        var made = (List<int>)Activator.CreateInstance(typeof(List<int>))!;
        made.Add(4);
        made.Add(2);
        var set = (HashSet<string>)Activator.CreateInstance(typeof(HashSet<string>))!;
        set.Add("k");
        set.Add("k");
        CheckStr("Activator-made framework collections accept writes",
            $"{dict.Count}|{dict["b"][0]}|{made.Count}|{made[1]}|{set.Count}|{((IDictionary)dict).Count}",
            "2|y|2|2|1|2");
    }
}
