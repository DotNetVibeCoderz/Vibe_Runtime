namespace RustNet.Deploy;

/// <summary>
/// Reads the debug information out of a compiled RNX image so a debugger can
/// map source lines to (method index, IL offset) breakpoint sites and back.
/// Mirrors the RNX layout in <c>runtime/rustnet-core/src/rnx.rs</c> (v1–v6).
/// </summary>
public sealed class RnxDebugInfo
{
    public sealed record MethodDbg(uint Index, string Name, IReadOnlyList<(uint Il, uint Line)> Points);

    public IReadOnlyList<string> Strings { get; }
    public IReadOnlyList<MethodDbg> Methods { get; }
    public uint? EntryMethod { get; }

    private readonly Dictionary<uint, MethodDbg> _byIndex;

    private RnxDebugInfo(List<string> strings, List<MethodDbg> methods, uint? entry)
    {
        Strings = strings;
        Methods = methods;
        EntryMethod = entry;
        _byIndex = methods.ToDictionary(m => m.Index);
    }

    /// <summary>Simple method name (`Ns.Type::Method` -&gt; `Method`) at an index.</summary>
    public string SimpleName(uint methodIndex)
    {
        if (!_byIndex.TryGetValue(methodIndex, out var m))
        {
            return $"method_{methodIndex}";
        }
        int c = m.Name.LastIndexOf("::", StringComparison.Ordinal);
        string after = c >= 0 ? m.Name[(c + 2)..] : m.Name;
        int paren = after.IndexOf('(');
        return paren >= 0 ? after[..paren] : after;
    }

    /// <summary>Source line for a paused (method, il) site, if known.</summary>
    public uint? LineAt(uint methodIndex, uint il)
    {
        if (!_byIndex.TryGetValue(methodIndex, out var m) || m.Points.Count == 0)
        {
            return null;
        }
        // The sequence point covering `il` is the last one at or before it.
        uint? line = null;
        foreach (var (pIl, pLine) in m.Points)
        {
            if (pIl <= il)
            {
                line = pLine;
            }
        }
        return line;
    }

    /// <summary>
    /// The breakpoint site for a source line: the first sequence point on that
    /// exact line, as (method index, IL offset). Null if no code is on the line.
    /// </summary>
    public (uint Method, uint Il)? SiteForLine(int line)
    {
        foreach (var m in Methods)
        {
            foreach (var (il, pLine) in m.Points)
            {
                if (pLine == (uint)line)
                {
                    return (m.Index, il);
                }
            }
        }
        return null;
    }

    /// <summary>Distinct source lines that carry executable code (for a client
    /// that wants to snap a requested breakpoint to the nearest valid line).</summary>
    public IReadOnlyList<int> ExecutableLines()
    {
        var set = new SortedSet<int>();
        foreach (var m in Methods)
        {
            foreach (var (_, line) in m.Points)
            {
                set.Add((int)line);
            }
        }
        return set.ToArray();
    }

    public static RnxDebugInfo Parse(byte[] rnx)
    {
        var r = new Cursor(rnx);
        if (r.Bytes(4) is not [(byte)'R', (byte)'N', (byte)'X', (byte)'1'])
        {
            throw new InvalidDataException("not an RNX image");
        }
        ushort version = r.U16();
        r.U16(); // flags
        r.U32(); // static_slot_count

        uint stringCount = r.U32();
        var strings = new List<string>((int)stringCount);
        for (uint i = 0; i < stringCount; i++)
        {
            uint len = r.U32();
            strings.Add(System.Text.Encoding.UTF8.GetString(r.Bytes((int)len)));
        }

        uint typeCount = r.U32();
        for (uint i = 0; i < typeCount; i++)
        {
            r.U32();               // name
            r.U16();               // field_count
            r.U16();               // static_field_count
            if (version >= 3)
            {
                r.U32();           // parent
                ushort ifaces = r.U16();
                for (int k = 0; k < ifaces; k++) r.U32();
                ushort overrides = r.U16();
                for (int k = 0; k < overrides; k++) { r.U32(); r.U32(); }
            }
            if (version >= 5)
            {
                ushort fields = r.U16();
                for (int k = 0; k < fields; k++) { r.U32(); r.U8(); r.U32(); }
            }
            if (version >= 6)
            {
                ushort attrs = r.U16();
                for (int k = 0; k < attrs; k++)
                {
                    r.U32();       // ctor
                    ushort fixedN = r.U16();
                    for (int a = 0; a < fixedN; a++) SkipAttrArg(r);
                    ushort namedN = r.U16();
                    for (int a = 0; a < namedN; a++) { r.U8(); r.U32(); SkipAttrArg(r); }
                }
            }
        }

        uint methodCount = r.U32();
        var names = new string[methodCount];
        for (uint i = 0; i < methodCount; i++)
        {
            uint nameIdx = r.U32();
            names[i] = nameIdx < strings.Count ? strings[(int)nameIdx] : $"method_{i}";
            r.U16();               // owner_type
            r.U8();                // flags
            r.U8();                // param_count
            r.U16();               // local_count
            r.U16();               // max_stack
            if (version >= 3) r.U32(); // slot
            uint codeLen = r.U32();
            r.Bytes((int)codeLen);
            if (version >= 2)
            {
                uint ehCount = r.U32();
                for (uint e = 0; e < ehCount; e++)
                {
                    r.U8();        // kind
                    r.U32(); r.U32(); r.U32(); r.U32(); // try/handler ranges
                    if (version >= 3) r.U32();          // filter_start
                }
            }
        }

        uint entryRaw = r.U32();
        uint? entry = entryRaw == 0xFFFF_FFFF ? null : entryRaw;

        var points = new Dictionary<uint, List<(uint, uint)>>();
        uint debugCount = r.U32();
        for (uint i = 0; i < debugCount; i++)
        {
            uint mi = r.U32();
            uint count = r.U32();
            var list = new List<(uint, uint)>((int)count);
            for (uint p = 0; p < count; p++)
            {
                uint il = r.U32();
                uint line = r.U32();
                list.Add((il, line));
            }
            points[mi] = list;
        }

        var methods = new List<MethodDbg>((int)methodCount);
        for (uint i = 0; i < methodCount; i++)
        {
            var pts = points.TryGetValue(i, out var l)
                ? (IReadOnlyList<(uint, uint)>)l
                : Array.Empty<(uint, uint)>();
            methods.Add(new MethodDbg(i, names[i], pts));
        }
        return new RnxDebugInfo(strings, methods, entry);
    }

    private static void SkipAttrArg(Cursor r)
    {
        byte tag = r.U8();
        switch (tag)
        {
            case 1: r.Bytes(4); break; // i32
            case 2: r.Bytes(8); break; // i64
            case 3: r.Bytes(8); break; // f64
            case 4: r.Bytes(4); break; // str idx
            case 5: r.Bytes(1); break; // bool
            // case 0: null, no payload
        }
    }

    private sealed class Cursor(byte[] data)
    {
        private int _pos;

        public byte[] Bytes(int n)
        {
            if (_pos + n > data.Length)
            {
                throw new InvalidDataException("unexpected end of RNX image");
            }
            byte[] s = data[_pos..(_pos + n)];
            _pos += n;
            return s;
        }

        public byte U8() => Bytes(1)[0];
        public ushort U16() => BitConverter.ToUInt16(Bytes(2), 0);
        public uint U32() => BitConverter.ToUInt32(Bytes(4), 0);
    }
}
