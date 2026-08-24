using System.Runtime.CompilerServices;

namespace RustNet.Security;

/// <summary>RFC 3986 percent-encoding (pure managed; runs on-device).</summary>
public static class Url
{
    private const string Unreserved =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.~";

    public static string Encode(string s)
    {
        System.Text.StringBuilder sb = new System.Text.StringBuilder();
        byte[] bytes = System.Text.Encoding.UTF8.GetBytes(s);
        for (int i = 0; i < bytes.Length; i++)
        {
            char c = (char)bytes[i];
            if (Unreserved.IndexOf(c) >= 0)
            {
                sb.Append(c);
            }
            else
            {
                sb.Append('%');
                sb.Append(HexUpper(bytes[i] >> 4));
                sb.Append(HexUpper(bytes[i] & 0xF));
            }
        }
        return sb.ToString();
    }

    private static char HexUpper(int nibble)
    {
        return (char)(nibble < 10 ? '0' + nibble : 'A' + (nibble - 10));
    }
}
