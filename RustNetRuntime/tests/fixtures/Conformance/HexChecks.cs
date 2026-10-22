using System;
using System.Globalization;
using System.Linq;

namespace Conformance;

// Reading colours written as hex: the digit tests and the parses a photo editor
// uses on "#80c0ff". Expected values were produced by dotnet.
static partial class Program
{
    static void HexChecks()
    {
        CheckStr("char.IsAsciiHexDigit and its cased forms",
            $"{char.IsAsciiHexDigit('a')}|{char.IsAsciiHexDigit('F')}|{char.IsAsciiHexDigit('g')}|{char.IsAsciiHexDigitLower('A')}|{char.IsAsciiHexDigitUpper('A')}|{char.IsAsciiLetterOrDigit('_')}",
            "True|True|False|False|True|False");
        var span = "#80c0ff".AsSpan(1);
        CheckStr("string.AsSpan reads the string", $"{span.Length}|{span[0]}|{span[5]}|{span.Slice(2, 2).ToString()}|{"abc".AsSpan(1, 1).ToString()}", "6|8|f|c0|b");
        CheckStr("Uri.IsHexDigit over a colour", string.Concat("#80c0ffz".Select(c => Uri.IsHexDigit(c) ? '1' : '0')), "01111110");
        var ok = int.TryParse("80c0ff", NumberStyles.HexNumber, CultureInfo.InvariantCulture, out var n);
        var spanOk = int.TryParse("80c0ff".AsSpan(), NumberStyles.HexNumber, null, out var m);
        CheckStr("int.TryParse with NumberStyles.HexNumber, string and span",
            $"{ok}|{n}|{spanOk}|{m}|{int.TryParse("zz", NumberStyles.HexNumber, null, out _)}|{int.Parse("FF", NumberStyles.HexNumber)}|{Convert.ToInt32("c0", 16)}",
            "True|8438015|True|8438015|False|255|192");
    }
}
