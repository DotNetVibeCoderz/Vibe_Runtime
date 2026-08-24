using System;
using System.Collections.Generic;
using System.Globalization;

namespace RustNet.Designer.Assistant;

/// <summary>
/// A small recursive-descent arithmetic evaluator, so the assistant can compute
/// timings, pixel budgets and colour maths instead of estimating them.
/// Supports <c>+ - * / % ^</c> with the usual precedence (<c>^</c> is
/// right-associative), unary sign, parentheses, the constants <c>pi</c> and
/// <c>e</c>, and a set of named functions. Throws
/// <see cref="FormatException"/> with the offending position on bad input.
/// </summary>
public static class Expression
{
    public static double Evaluate(string input)
    {
        var parser = new Parser(input);
        double value = parser.ParseExpression();
        parser.ExpectEnd();
        return value;
    }

    private sealed class Parser
    {
        private readonly string _s;
        private int _i;

        public Parser(string s) => _s = s ?? "";

        public double ParseExpression()
        {
            double left = ParseTerm();
            while (true)
            {
                SkipSpace();
                if (Match('+'))
                {
                    left += ParseTerm();
                }
                else if (Match('-'))
                {
                    left -= ParseTerm();
                }
                else
                {
                    return left;
                }
            }
        }

        private double ParseTerm()
        {
            double left = ParsePower();
            while (true)
            {
                SkipSpace();
                if (Match('*'))
                {
                    left *= ParsePower();
                }
                else if (Match('/'))
                {
                    double d = ParsePower();
                    if (d == 0)
                    {
                        throw new DivideByZeroException("Division by zero");
                    }
                    left /= d;
                }
                else if (Match('%'))
                {
                    double d = ParsePower();
                    if (d == 0)
                    {
                        throw new DivideByZeroException("Modulo by zero");
                    }
                    left %= d;
                }
                else
                {
                    return left;
                }
            }
        }

        private double ParsePower()
        {
            double b = ParseUnary();
            SkipSpace();
            // Right-associative: 2^3^2 is 2^(3^2).
            return Match('^') ? Math.Pow(b, ParsePower()) : b;
        }

        private double ParseUnary()
        {
            SkipSpace();
            // The sign binds looser than '^', so -2^2 is -(2^2) = -4, matching
            // how a calculator reads it.
            if (Match('-'))
            {
                return -ParsePower();
            }
            if (Match('+'))
            {
                return ParsePower();
            }
            return ParsePrimary();
        }

        private double ParsePrimary()
        {
            SkipSpace();
            if (_i >= _s.Length)
            {
                throw new FormatException("Unexpected end of expression");
            }

            if (Match('('))
            {
                double v = ParseExpression();
                SkipSpace();
                if (!Match(')'))
                {
                    throw new FormatException($"Expected ')' at position {_i}");
                }
                return v;
            }

            char c = _s[_i];
            if (char.IsAsciiDigit(c) || c == '.')
            {
                return ParseNumber();
            }
            if (char.IsAsciiLetter(c) || c == '_')
            {
                return ParseIdentifier();
            }
            throw new FormatException($"Unexpected '{c}' at position {_i}");
        }

        private double ParseNumber()
        {
            int start = _i;
            while (_i < _s.Length && (char.IsAsciiDigit(_s[_i]) || _s[_i] == '.'))
            {
                _i++;
            }
            // Exponent form: 1e-3, 2.5E6.
            if (_i < _s.Length && (_s[_i] == 'e' || _s[_i] == 'E')
                && _i + 1 < _s.Length
                && (char.IsAsciiDigit(_s[_i + 1]) || _s[_i + 1] == '-' || _s[_i + 1] == '+'))
            {
                _i += 2;
                while (_i < _s.Length && char.IsAsciiDigit(_s[_i]))
                {
                    _i++;
                }
            }
            string text = _s.Substring(start, _i - start);
            if (!double.TryParse(text, NumberStyles.Float, CultureInfo.InvariantCulture, out double v))
            {
                throw new FormatException($"'{text}' is not a number");
            }
            return v;
        }

        private double ParseIdentifier()
        {
            int start = _i;
            while (_i < _s.Length && (char.IsAsciiLetterOrDigit(_s[_i]) || _s[_i] == '_'))
            {
                _i++;
            }
            string name = _s.Substring(start, _i - start).ToLowerInvariant();

            SkipSpace();
            if (!Match('('))
            {
                return name switch
                {
                    "pi" => Math.PI,
                    "e" => Math.E,
                    "tau" => Math.Tau,
                    _ => throw new FormatException($"Unknown name '{name}'"),
                };
            }

            var args = new List<double>();
            SkipSpace();
            if (!Match(')'))
            {
                do
                {
                    args.Add(ParseExpression());
                    SkipSpace();
                }
                while (Match(','));
                SkipSpace();
                if (!Match(')'))
                {
                    throw new FormatException($"Expected ')' closing {name}( at position {_i}");
                }
            }
            return Apply(name, args);
        }

        private static double Apply(string name, List<double> a)
        {
            double One() => a.Count == 1 ? a[0]
                : throw new FormatException($"{name}() takes 1 argument, got {a.Count}");
            double[] Two() => a.Count == 2 ? new[] { a[0], a[1] }
                : throw new FormatException($"{name}() takes 2 arguments, got {a.Count}");

            switch (name)
            {
                case "abs": return Math.Abs(One());
                case "sqrt": return Math.Sqrt(One());
                case "cbrt": return Math.Cbrt(One());
                case "exp": return Math.Exp(One());
                case "ln": return Math.Log(One());
                case "log": return a.Count == 2 ? Math.Log(a[0], a[1]) : Math.Log10(One());
                case "log2": return Math.Log2(One());
                case "log10": return Math.Log10(One());
                case "sin": return Math.Sin(One());
                case "cos": return Math.Cos(One());
                case "tan": return Math.Tan(One());
                case "asin": return Math.Asin(One());
                case "acos": return Math.Acos(One());
                case "atan": return Math.Atan(One());
                case "atan2": { double[] t = Two(); return Math.Atan2(t[0], t[1]); }
                case "sinh": return Math.Sinh(One());
                case "cosh": return Math.Cosh(One());
                case "tanh": return Math.Tanh(One());
                case "floor": return Math.Floor(One());
                case "ceil": return Math.Ceiling(One());
                case "round": return a.Count == 2
                    ? Math.Round(a[0], (int)a[1], MidpointRounding.AwayFromZero)
                    : Math.Round(One(), MidpointRounding.AwayFromZero);
                case "trunc": return Math.Truncate(One());
                case "sign": return Math.Sign(One());
                case "pow": { double[] t = Two(); return Math.Pow(t[0], t[1]); }
                case "hypot": { double[] t = Two(); return Math.Sqrt(t[0] * t[0] + t[1] * t[1]); }
                case "deg": return One() * 180.0 / Math.PI;
                case "rad": return One() * Math.PI / 180.0;
                case "min":
                case "max":
                {
                    if (a.Count == 0)
                    {
                        throw new FormatException($"{name}() needs at least 1 argument");
                    }
                    double acc = a[0];
                    for (int i = 1; i < a.Count; i++)
                    {
                        acc = name == "min" ? Math.Min(acc, a[i]) : Math.Max(acc, a[i]);
                    }
                    return acc;
                }
                case "sum":
                {
                    double acc = 0;
                    foreach (double v in a)
                    {
                        acc += v;
                    }
                    return acc;
                }
                case "avg":
                {
                    if (a.Count == 0)
                    {
                        throw new FormatException("avg() needs at least 1 argument");
                    }
                    double acc = 0;
                    foreach (double v in a)
                    {
                        acc += v;
                    }
                    return acc / a.Count;
                }
                default:
                    throw new FormatException($"Unknown function '{name}'");
            }
        }

        public void ExpectEnd()
        {
            SkipSpace();
            if (_i < _s.Length)
            {
                throw new FormatException($"Unexpected '{_s[_i]}' at position {_i}");
            }
        }

        private void SkipSpace()
        {
            while (_i < _s.Length && char.IsWhiteSpace(_s[_i]))
            {
                _i++;
            }
        }

        private bool Match(char c)
        {
            if (_i < _s.Length && _s[_i] == c)
            {
                _i++;
                return true;
            }
            return false;
        }
    }
}
