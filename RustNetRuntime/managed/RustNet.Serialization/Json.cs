using System.Collections.Generic;
using System.Text;

namespace RustNet.Serialization;

/// <summary>
/// JSON document node (reflection-free DOM: build, parse and write without
/// runtime type inspection, so it runs on the device interpreter).
/// </summary>
public class JsonValue
{
    public const int NullKind = 0;
    public const int BoolKind = 1;
    public const int NumberKind = 2;
    public const int StringKind = 3;
    public const int ArrayKind = 4;
    public const int ObjectKind = 5;

    public int Kind;
    public bool Flag;
    public double Number;
    public string Text = "";
    public List<JsonValue> Items = new List<JsonValue>();
    public List<string> Keys = new List<string>();

    // ---- constructors ----

    public static JsonValue Null()
    {
        return new JsonValue();
    }

    public static JsonValue FromBool(bool b)
    {
        JsonValue v = new JsonValue();
        v.Kind = BoolKind;
        v.Flag = b;
        return v;
    }

    public static JsonValue FromNumber(double n)
    {
        JsonValue v = new JsonValue();
        v.Kind = NumberKind;
        v.Number = n;
        return v;
    }

    public static JsonValue FromString(string s)
    {
        JsonValue v = new JsonValue();
        v.Kind = StringKind;
        v.Text = s;
        return v;
    }

    public static JsonValue NewArray()
    {
        JsonValue v = new JsonValue();
        v.Kind = ArrayKind;
        return v;
    }

    public static JsonValue NewObject()
    {
        JsonValue v = new JsonValue();
        v.Kind = ObjectKind;
        return v;
    }

    // ---- accessors ----

    public int Count => Items.Count;
    public bool IsNull => Kind == NullKind;
    public bool AsBool => Kind == BoolKind ? Flag : Kind == NumberKind && Number != 0;
    public double AsDouble => Number;
    public int AsInt => (int)Number;
    public long AsLong => (long)Number;
    public string AsString => Kind == StringKind ? Text : ToJson();

    /// <summary>Array element by index.</summary>
    public JsonValue At(int index) => Items[index];

    /// <summary>Object member by key, or a Null node when absent.</summary>
    public JsonValue Get(string key)
    {
        for (int i = 0; i < Keys.Count; i++)
        {
            if (Keys[i] == key)
            {
                return Items[i];
            }
        }
        return Null();
    }

    public bool Has(string key)
    {
        for (int i = 0; i < Keys.Count; i++)
        {
            if (Keys[i] == key)
            {
                return true;
            }
        }
        return false;
    }

    public JsonValue Add(JsonValue item)
    {
        Items.Add(item);
        return this;
    }

    public JsonValue Set(string key, JsonValue value)
    {
        for (int i = 0; i < Keys.Count; i++)
        {
            if (Keys[i] == key)
            {
                Items[i] = value;
                return this;
            }
        }
        Keys.Add(key);
        Items.Add(value);
        return this;
    }

    public JsonValue Set(string key, string value) => Set(key, FromString(value));
    public JsonValue Set(string key, double value) => Set(key, FromNumber(value));
    public JsonValue Set(string key, bool value) => Set(key, FromBool(value));

    // ---- writer ----

    public string ToJson()
    {
        StringBuilder sb = new StringBuilder();
        WriteTo(sb);
        return sb.ToString();
    }

    private void WriteTo(StringBuilder sb)
    {
        if (Kind == NullKind)
        {
            sb.Append("null");
        }
        else if (Kind == BoolKind)
        {
            sb.Append(Flag ? "true" : "false");
        }
        else if (Kind == NumberKind)
        {
            long whole = (long)Number;
            if (whole == Number)
            {
                sb.Append(whole.ToString());
            }
            else
            {
                sb.Append(Number.ToString());
            }
        }
        else if (Kind == StringKind)
        {
            WriteEscaped(sb, Text);
        }
        else if (Kind == ArrayKind)
        {
            sb.Append('[');
            for (int i = 0; i < Items.Count; i++)
            {
                if (i > 0)
                {
                    sb.Append(',');
                }
                Items[i].WriteTo(sb);
            }
            sb.Append(']');
        }
        else
        {
            sb.Append('{');
            for (int i = 0; i < Keys.Count; i++)
            {
                if (i > 0)
                {
                    sb.Append(',');
                }
                WriteEscaped(sb, Keys[i]);
                sb.Append(':');
                Items[i].WriteTo(sb);
            }
            sb.Append('}');
        }
    }

    private static void WriteEscaped(StringBuilder sb, string s)
    {
        sb.Append('"');
        for (int i = 0; i < s.Length; i++)
        {
            char c = s[i];
            if (c == '"')
            {
                sb.Append("\\\"");
            }
            else if (c == '\\')
            {
                sb.Append("\\\\");
            }
            else if (c == '\n')
            {
                sb.Append("\\n");
            }
            else if (c == '\r')
            {
                sb.Append("\\r");
            }
            else if (c == '\t')
            {
                sb.Append("\\t");
            }
            else
            {
                sb.Append(c);
            }
        }
        sb.Append('"');
    }
}

/// <summary>JSON parser producing <see cref="JsonValue"/> trees.</summary>
public class Json
{
    private string _s = "";
    private int _pos;

    public static JsonValue Parse(string text)
    {
        Json p = new Json();
        p._s = text;
        p._pos = 0;
        p.SkipWs();
        JsonValue v = p.ParseValue();
        return v;
    }

    private void SkipWs()
    {
        while (_pos < _s.Length)
        {
            char c = _s[_pos];
            if (c == ' ' || c == '\t' || c == '\n' || c == '\r')
            {
                _pos = _pos + 1;
            }
            else
            {
                break;
            }
        }
    }

    private JsonValue ParseValue()
    {
        char c = _s[_pos];
        if (c == '{')
        {
            return ParseObject();
        }
        if (c == '[')
        {
            return ParseArray();
        }
        if (c == '"')
        {
            return JsonValue.FromString(ParseString());
        }
        if (c == 't')
        {
            _pos = _pos + 4;
            return JsonValue.FromBool(true);
        }
        if (c == 'f')
        {
            _pos = _pos + 5;
            return JsonValue.FromBool(false);
        }
        if (c == 'n')
        {
            _pos = _pos + 4;
            return JsonValue.Null();
        }
        return ParseNumber();
    }

    private JsonValue ParseObject()
    {
        JsonValue obj = JsonValue.NewObject();
        _pos = _pos + 1; // {
        SkipWs();
        if (_pos < _s.Length && _s[_pos] == '}')
        {
            _pos = _pos + 1;
            return obj;
        }
        while (true)
        {
            SkipWs();
            string key = ParseString();
            SkipWs();
            _pos = _pos + 1; // :
            SkipWs();
            obj.Set(key, ParseValue());
            SkipWs();
            if (_pos < _s.Length && _s[_pos] == ',')
            {
                _pos = _pos + 1;
            }
            else
            {
                break;
            }
        }
        _pos = _pos + 1; // }
        return obj;
    }

    private JsonValue ParseArray()
    {
        JsonValue arr = JsonValue.NewArray();
        _pos = _pos + 1; // [
        SkipWs();
        if (_pos < _s.Length && _s[_pos] == ']')
        {
            _pos = _pos + 1;
            return arr;
        }
        while (true)
        {
            SkipWs();
            arr.Add(ParseValue());
            SkipWs();
            if (_pos < _s.Length && _s[_pos] == ',')
            {
                _pos = _pos + 1;
            }
            else
            {
                break;
            }
        }
        _pos = _pos + 1; // ]
        return arr;
    }

    private string ParseString()
    {
        StringBuilder sb = new StringBuilder();
        _pos = _pos + 1; // opening quote
        while (_pos < _s.Length)
        {
            char c = _s[_pos];
            if (c == '"')
            {
                _pos = _pos + 1;
                break;
            }
            if (c == '\\')
            {
                _pos = _pos + 1;
                char e = _s[_pos];
                if (e == 'n')
                {
                    sb.Append('\n');
                }
                else if (e == 'r')
                {
                    sb.Append('\r');
                }
                else if (e == 't')
                {
                    sb.Append('\t');
                }
                else if (e == 'u')
                {
                    int code = 0;
                    for (int i = 1; i <= 4; i++)
                    {
                        code = code * 16 + HexDigit(_s[_pos + i]);
                    }
                    sb.Append((char)code);
                    _pos = _pos + 4;
                }
                else
                {
                    sb.Append(e);
                }
                _pos = _pos + 1;
            }
            else
            {
                sb.Append(c);
                _pos = _pos + 1;
            }
        }
        return sb.ToString();
    }

    private static int HexDigit(char c)
    {
        if (c >= '0' && c <= '9')
        {
            return c - '0';
        }
        if (c >= 'a' && c <= 'f')
        {
            return c - 'a' + 10;
        }
        return c - 'A' + 10;
    }

    private JsonValue ParseNumber()
    {
        int start = _pos;
        while (_pos < _s.Length)
        {
            char c = _s[_pos];
            if ((c >= '0' && c <= '9') || c == '-' || c == '+' || c == '.' || c == 'e' || c == 'E')
            {
                _pos = _pos + 1;
            }
            else
            {
                break;
            }
        }
        string text = _s.Substring(start, _pos - start);
        return JsonValue.FromNumber(double.Parse(text));
    }
}
