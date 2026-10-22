namespace Conformance;

// An interface implemented explicitly by a generic type, called through a boxed
// instance: Rustzor's renderer reads `IEventCallbackLike.Untyped` from an
// `EventCallback<T>` that arrived as object (an @onclick on a NavLink).
// Expected values were produced by dotnet.
interface IUntyped
{
    string Untyped { get; }
    int Twice(int x);
}

readonly struct Callback<T> : IUntyped
{
    private readonly string _label;
    public Callback(string label) => _label = label;
    string IUntyped.Untyped => _label + ":" + typeof(T).Name;
    int IUntyped.Twice(int x) => x * 2;
}

sealed class Holder<T> : IUntyped
{
    string IUntyped.Untyped => "holder:" + typeof(T).Name;
    int IUntyped.Twice(int x) => x + x + 1;
}

static partial class Program
{
    static string ReadUntyped(object value) => value is IUntyped u ? u.Untyped + "/" + u.Twice(21) : "none";

    static void ExplicitGenericChecks()
    {
        CheckStr("explicit interface members on a boxed generic struct",
            ReadUntyped(new Callback<int>("click")) + "|" + ReadUntyped(new Callback<string>("input")),
            "click:Int32/42|input:String/42");
        CheckStr("explicit interface members on a generic class",
            ReadUntyped(new Holder<double>()) + "|" + ReadUntyped("not one"),
            "holder:Double/43|none");
    }
}
