using System.Collections.Generic;
using System.Text;

namespace RustNet.Serialization;

/// <summary>
/// Compact binary encoding of <see cref="JsonValue"/> documents — the same
/// document model as JSON, roughly half the wire size and much cheaper to
/// parse. Format: tag byte, then payload (numbers little-endian, strings
/// length-prefixed UTF-8).
/// </summary>
public static class BinarySerializer
{
    private const byte TagNull = 0;
    private const byte TagFalse = 1;
    private const byte TagTrue = 2;
    private const byte TagInt = 3; // i64
    private const byte TagReal = 4; // f64
    private const byte TagString = 5;
    private const byte TagArray = 6;
    private const byte TagObject = 7;

    public static byte[] Serialize(JsonValue doc)
    {
        List<byte> outBytes = new List<byte>();
        Write(outBytes, doc);
        byte[] result = new byte[outBytes.Count];
        for (int i = 0; i < result.Length; i++)
        {
            result[i] = outBytes[i];
        }
        return result;
    }

    private static void Write(List<byte> o, JsonValue v)
    {
        if (v.Kind == JsonValue.NullKind)
        {
            o.Add(TagNull);
        }
        else if (v.Kind == JsonValue.BoolKind)
        {
            o.Add(v.Flag ? TagTrue : TagFalse);
        }
        else if (v.Kind == JsonValue.NumberKind)
        {
            long whole = (long)v.Number;
            if (whole == v.Number)
            {
                o.Add(TagInt);
                WriteLong(o, whole);
            }
            else
            {
                o.Add(TagReal);
                WriteLong(o, System.BitConverter.DoubleToInt64Bits(v.Number));
            }
        }
        else if (v.Kind == JsonValue.StringKind)
        {
            o.Add(TagString);
            WriteString(o, v.Text);
        }
        else if (v.Kind == JsonValue.ArrayKind)
        {
            o.Add(TagArray);
            WriteInt(o, v.Items.Count);
            for (int i = 0; i < v.Items.Count; i++)
            {
                Write(o, v.Items[i]);
            }
        }
        else
        {
            o.Add(TagObject);
            WriteInt(o, v.Keys.Count);
            for (int i = 0; i < v.Keys.Count; i++)
            {
                WriteString(o, v.Keys[i]);
                Write(o, v.Items[i]);
            }
        }
    }

    private static void WriteInt(List<byte> o, int v)
    {
        o.Add((byte)v);
        o.Add((byte)(v >> 8));
        o.Add((byte)(v >> 16));
        o.Add((byte)(v >> 24));
    }

    private static void WriteLong(List<byte> o, long v)
    {
        for (int i = 0; i < 8; i++)
        {
            o.Add((byte)(v >> (8 * i)));
        }
    }

    private static void WriteString(List<byte> o, string s)
    {
        byte[] utf8 = Encoding.UTF8.GetBytes(s);
        WriteInt(o, utf8.Length);
        for (int i = 0; i < utf8.Length; i++)
        {
            o.Add(utf8[i]);
        }
    }

    // ---- reader ----

    public static JsonValue Deserialize(byte[] data)
    {
        BinaryReaderState r = new BinaryReaderState(data);
        return r.Read();
    }
}

/// <summary>Cursor over serialized bytes (state object instead of ref params).</summary>
public class BinaryReaderState
{
    private readonly byte[] _d;
    private int _pos;

    public BinaryReaderState(byte[] data)
    {
        _d = data;
        _pos = 0;
    }

    public JsonValue Read()
    {
        byte tag = _d[_pos];
        _pos = _pos + 1;
        if (tag == 0)
        {
            return JsonValue.Null();
        }
        if (tag == 1)
        {
            return JsonValue.FromBool(false);
        }
        if (tag == 2)
        {
            return JsonValue.FromBool(true);
        }
        if (tag == 3)
        {
            return JsonValue.FromNumber(ReadLong());
        }
        if (tag == 4)
        {
            return JsonValue.FromNumber(System.BitConverter.Int64BitsToDouble(ReadLong()));
        }
        if (tag == 5)
        {
            return JsonValue.FromString(ReadString());
        }
        if (tag == 6)
        {
            int n = ReadInt();
            JsonValue arr = JsonValue.NewArray();
            for (int i = 0; i < n; i++)
            {
                arr.Add(Read());
            }
            return arr;
        }
        int count = ReadInt();
        JsonValue obj = JsonValue.NewObject();
        for (int i = 0; i < count; i++)
        {
            string key = ReadString();
            obj.Set(key, Read());
        }
        return obj;
    }

    private int ReadInt()
    {
        int v = _d[_pos] | (_d[_pos + 1] << 8) | (_d[_pos + 2] << 16) | (_d[_pos + 3] << 24);
        _pos = _pos + 4;
        return v;
    }

    private long ReadLong()
    {
        long v = 0;
        for (int i = 7; i >= 0; i--)
        {
            v = (v << 8) | _d[_pos + i];
        }
        _pos = _pos + 8;
        return v;
    }

    private string ReadString()
    {
        int len = ReadInt();
        byte[] utf8 = new byte[len];
        for (int i = 0; i < len; i++)
        {
            utf8[i] = _d[_pos + i];
        }
        _pos = _pos + len;
        return Encoding.UTF8.GetString(utf8);
    }
}
