using System.Collections.Generic;
using System.Text;

namespace RustNet.Serialization;

/// <summary>
/// XML element node: name, attributes (parallel name/value lists), text
/// content and child elements. Namespace-free subset, good for config and
/// UI markup files.
/// </summary>
public class XmlNode
{
    public string Name = "";
    public string Text = "";
    public List<string> AttrNames = new List<string>();
    public List<string> AttrValues = new List<string>();
    public List<XmlNode> Children = new List<XmlNode>();

    public string GetAttr(string name)
    {
        for (int i = 0; i < AttrNames.Count; i++)
        {
            if (AttrNames[i] == name)
            {
                return AttrValues[i];
            }
        }
        return "";
    }

    public bool HasAttr(string name)
    {
        for (int i = 0; i < AttrNames.Count; i++)
        {
            if (AttrNames[i] == name)
            {
                return true;
            }
        }
        return false;
    }

    public XmlNode SetAttr(string name, string value)
    {
        for (int i = 0; i < AttrNames.Count; i++)
        {
            if (AttrNames[i] == name)
            {
                AttrValues[i] = value;
                return this;
            }
        }
        AttrNames.Add(name);
        AttrValues.Add(value);
        return this;
    }

    /// <summary>First child element with the given name, or null.</summary>
    public XmlNode Child(string name)
    {
        for (int i = 0; i < Children.Count; i++)
        {
            if (Children[i].Name == name)
            {
                return Children[i];
            }
        }
        return null;
    }

    public string ToXml()
    {
        StringBuilder sb = new StringBuilder();
        WriteTo(sb);
        return sb.ToString();
    }

    private void WriteTo(StringBuilder sb)
    {
        sb.Append('<');
        sb.Append(Name);
        for (int i = 0; i < AttrNames.Count; i++)
        {
            sb.Append(' ');
            sb.Append(AttrNames[i]);
            sb.Append("=\"");
            sb.Append(Escape(AttrValues[i]));
            sb.Append('"');
        }
        if (Children.Count == 0 && Text.Length == 0)
        {
            sb.Append("/>");
            return;
        }
        sb.Append('>');
        sb.Append(Escape(Text));
        for (int i = 0; i < Children.Count; i++)
        {
            Children[i].WriteTo(sb);
        }
        sb.Append("</");
        sb.Append(Name);
        sb.Append('>');
    }

    private static string Escape(string s)
    {
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < s.Length; i++)
        {
            char c = s[i];
            if (c == '<')
            {
                sb.Append("&lt;");
            }
            else if (c == '>')
            {
                sb.Append("&gt;");
            }
            else if (c == '&')
            {
                sb.Append("&amp;");
            }
            else if (c == '"')
            {
                sb.Append("&quot;");
            }
            else
            {
                sb.Append(c);
            }
        }
        return sb.ToString();
    }
}

/// <summary>Small XML parser producing <see cref="XmlNode"/> trees.</summary>
public class Xml
{
    private string _s = "";
    private int _pos;

    public static XmlNode Parse(string text)
    {
        Xml p = new Xml();
        p._s = text;
        p._pos = 0;
        p.SkipMisc();
        return p.ParseElement();
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

    /// <summary>Skip whitespace, declarations (&lt;?...?&gt;) and comments.</summary>
    private void SkipMisc()
    {
        while (true)
        {
            SkipWs();
            if (_pos + 1 < _s.Length && _s[_pos] == '<' && _s[_pos + 1] == '?')
            {
                while (_pos < _s.Length && _s[_pos] != '>')
                {
                    _pos = _pos + 1;
                }
                _pos = _pos + 1;
            }
            else if (_pos + 3 < _s.Length && _s[_pos] == '<' && _s[_pos + 1] == '!' && _s[_pos + 2] == '-' && _s[_pos + 3] == '-')
            {
                _pos = _pos + 4;
                while (_pos + 2 < _s.Length && !(_s[_pos] == '-' && _s[_pos + 1] == '-' && _s[_pos + 2] == '>'))
                {
                    _pos = _pos + 1;
                }
                _pos = _pos + 3;
            }
            else
            {
                break;
            }
        }
    }

    private XmlNode ParseElement()
    {
        XmlNode node = new XmlNode();
        _pos = _pos + 1; // <
        node.Name = ParseName();
        while (true)
        {
            SkipWs();
            char c = _s[_pos];
            if (c == '/')
            {
                _pos = _pos + 2; // />
                return node;
            }
            if (c == '>')
            {
                _pos = _pos + 1;
                break;
            }
            string attr = ParseName();
            SkipWs();
            _pos = _pos + 1; // =
            SkipWs();
            char quote = _s[_pos];
            _pos = _pos + 1;
            StringBuilder val = new StringBuilder();
            while (_pos < _s.Length && _s[_pos] != quote)
            {
                val.Append(_s[_pos]);
                _pos = _pos + 1;
            }
            _pos = _pos + 1;
            node.SetAttr(attr, Unescape(val.ToString()));
        }
        // content: text and child elements until </name>
        StringBuilder text = new StringBuilder();
        while (_pos < _s.Length)
        {
            if (_s[_pos] == '<')
            {
                if (_pos + 1 < _s.Length && _s[_pos + 1] == '/')
                {
                    while (_pos < _s.Length && _s[_pos] != '>')
                    {
                        _pos = _pos + 1;
                    }
                    _pos = _pos + 1;
                    break;
                }
                if (_pos + 3 < _s.Length && _s[_pos + 1] == '!' && _s[_pos + 2] == '-' && _s[_pos + 3] == '-')
                {
                    SkipMisc();
                    continue;
                }
                node.Children.Add(ParseElement());
            }
            else
            {
                text.Append(_s[_pos]);
                _pos = _pos + 1;
            }
        }
        node.Text = Unescape(text.ToString()).Trim();
        return node;
    }

    private string ParseName()
    {
        StringBuilder sb = new StringBuilder();
        while (_pos < _s.Length)
        {
            char c = _s[_pos];
            if (char.IsLetterOrDigit(c) || c == '_' || c == '-' || c == ':' || c == '.')
            {
                sb.Append(c);
                _pos = _pos + 1;
            }
            else
            {
                break;
            }
        }
        return sb.ToString();
    }

    private static string Unescape(string s)
    {
        if (s.IndexOf('&') < 0)
        {
            return s;
        }
        StringBuilder sb = new StringBuilder();
        int i = 0;
        while (i < s.Length)
        {
            if (s[i] == '&')
            {
                if (i + 3 < s.Length && s[i + 1] == 'l' && s[i + 2] == 't' && s[i + 3] == ';')
                {
                    sb.Append('<');
                    i = i + 4;
                    continue;
                }
                if (i + 3 < s.Length && s[i + 1] == 'g' && s[i + 2] == 't' && s[i + 3] == ';')
                {
                    sb.Append('>');
                    i = i + 4;
                    continue;
                }
                if (i + 4 < s.Length && s[i + 1] == 'a' && s[i + 2] == 'm' && s[i + 3] == 'p' && s[i + 4] == ';')
                {
                    sb.Append('&');
                    i = i + 5;
                    continue;
                }
                if (i + 5 < s.Length && s[i + 1] == 'q' && s[i + 2] == 'u' && s[i + 3] == 'o' && s[i + 4] == 't' && s[i + 5] == ';')
                {
                    sb.Append('"');
                    i = i + 6;
                    continue;
                }
            }
            sb.Append(s[i]);
            i = i + 1;
        }
        return sb.ToString();
    }
}
