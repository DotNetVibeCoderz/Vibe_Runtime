using System;
using System.Collections.Generic;
using System.Linq;
using System.Text;

namespace Conformance;

// Text with emoji in it, rebuilt one char at a time, and chars that travel
// as objects. Expected values were produced by dotnet.
static partial class Program
{
    static void TextChecks()
    {
        var s = "a😊b";
        var viaToString = new StringBuilder();
        foreach (var ch in s) viaToString.Append(ch.ToString());
        var viaChar = new StringBuilder();
        foreach (var ch in s) viaChar.Append(ch);
        CheckStr("surrogate pairs survive char-by-char rebuilding",
            $"{viaToString.ToString() == s}|{viaChar.ToString() == s}|{s[1].ToString().Length}|{new string(s.ToCharArray()) == s}|{string.Concat(s.Select(c => c)) == s}",
            "True|True|1|True|True");
        CheckStr("surrogate helpers",
            $"{char.ConvertFromUtf32(0x1F60A) == "😊"}|{char.ConvertToUtf32(s, 1):x}|{char.IsSurrogate(s[1])}|{char.IsHighSurrogate(s, 1)}|{char.IsLowSurrogate(s[2])}|{char.IsSurrogatePair(s[1], s[2])}|{char.IsSurrogate('a')}",
            "True|1f60a|True|True|True|True|False");
        object boxedChar = 'x', boxedBool = true;
        CheckStr("boxed chars and bools print as themselves",
            $"{boxedChar}|{string.Concat('x', 'y')}|{string.Concat("a", 'y')}|{boxedBool}|{string.Concat(boxedChar, boxedBool)}",
            "x|xy|ay|True|xTrue");
        var utf8 = Encoding.UTF8.GetBytes("xKopi☕y");
        var copy = new byte[3];
        Array.Copy(utf8, 1, copy, 0, 3);
        CheckStr("bytes as a wire protocol reads them",
            $"{Encoding.UTF8.GetString(utf8, 1, 7)}|{Encoding.UTF8.GetString(copy)}|{BitConverter.Int64BitsToDouble(BitConverter.DoubleToInt64Bits(85000.5))}|{BitConverter.DoubleToInt64Bits(1.0):x}|{BitConverter.Int32BitsToSingle(BitConverter.SingleToInt32Bits(2.5f))}|{Convert.ToBase64String(Convert.FromBase64String("AAEC/w=="))}",
            "Kopi☕|Kop|85000.5|3ff0000000000000|2.5|AAEC/w==");
        var nums = new List<int> { 3, 1, 2 };
        CheckStr("AsEnumerable keeps the sequence",
            $"{string.Join(",", nums.AsEnumerable().Reverse())}|{nums.AsEnumerable().Count()}",
            "2,1,3|3");
    }
}
