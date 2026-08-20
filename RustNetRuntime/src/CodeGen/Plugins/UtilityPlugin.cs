using System.ComponentModel;
using System.Globalization;
using Microsoft.SemanticKernel;

namespace CodeGen.Plugins;

/// <summary>
/// Arithmetic and clock access.
///
/// Language models are unreliable at arithmetic and have no clock, so both are
/// given as tools rather than left to the model.
/// </summary>
public sealed class UtilityPlugin(Action<string> log)
{
    [KernelFunction("math_calculation")]
    [Description("Evaluate an arithmetic expression and return the result. Supports + - * / % ^, parentheses, and the functions sqrt, abs, min, max, pow, floor, ceil, round, log, ln, exp, sin, cos, tan.")]
    public string Calculate(
        [Description("The expression, for example '(1920 * 1080) / 2' or 'sqrt(2) * 100'.")] string expression)
    {
        if (string.IsNullOrWhiteSpace(expression)) return "Give me an expression to evaluate.";

        try
        {
            var value = ExpressionEvaluator.Evaluate(expression);
            log($"Jack calculated {expression.Trim()} = {value}");
            return value.ToString("G15", CultureInfo.InvariantCulture);
        }
        catch (FormatException ex)
        {
            return $"Could not evaluate '{expression}': {ex.Message}";
        }
        catch (DivideByZeroException)
        {
            return "Division by zero.";
        }
    }

    [KernelFunction("current_date_time")]
    [Description("Get the current date and time. Use this instead of guessing today's date.")]
    public string CurrentDateTime(
        [Description("Set to true for UTC rather than local time.")] bool utc = false)
    {
        var now = utc ? DateTimeOffset.UtcNow : DateTimeOffset.Now;
        return string.Join('\n',
            $"ISO 8601   {now:yyyy-MM-ddTHH:mm:sszzz}",
            $"Date       {now:dddd, d MMMM yyyy}",
            $"Time       {now:HH:mm:ss}",
            $"Zone       {(utc ? "UTC" : TimeZoneInfo.Local.StandardName)}",
            $"Unix       {now.ToUnixTimeSeconds()}");
    }

    [KernelFunction("date_difference")]
    [Description("Work out the time between two dates. Give both as ISO dates, for example 2026-01-31.")]
    public string DateDifference(
        [Description("The earlier date.")] string from,
        [Description("The later date. Leave empty for today.")] string to = "")
    {
        if (!DateTime.TryParse(from, CultureInfo.InvariantCulture, DateTimeStyles.None, out var start))
        {
            return $"Could not read '{from}' as a date.";
        }

        var end = DateTime.Today;
        if (!string.IsNullOrWhiteSpace(to)
            && !DateTime.TryParse(to, CultureInfo.InvariantCulture, DateTimeStyles.None, out end))
        {
            return $"Could not read '{to}' as a date.";
        }

        var span = end - start;
        return string.Join('\n',
            $"From       {start:yyyy-MM-dd}",
            $"To         {end:yyyy-MM-dd}",
            $"Days       {span.TotalDays:0}",
            $"Weeks      {span.TotalDays / 7:0.##}",
            $"Hours      {span.TotalHours:0}");
    }
}

/// <summary>
/// A small recursive-descent expression evaluator.
///
/// Written out rather than delegated to <c>DataTable.Compute</c>: that helper
/// parses with the current culture, has its own operator set, and reports
/// failures as an opaque <c>SyntaxErrorException</c>. This version is
/// culture-invariant and says what it did not understand.
/// </summary>
internal static class ExpressionEvaluator
{
    public static double Evaluate(string expression)
    {
        var parser = new Parser(expression);
        var value = parser.ParseExpression();
        parser.ExpectEnd();
        return value;
    }

    private sealed class Parser(string text)
    {
        private readonly string _text = text;
        private int _position;

        public double ParseExpression() => ParseAdditive();

        /// <summary>Fails if anything is left over, which catches typos like `2 3`.</summary>
        public void ExpectEnd()
        {
            SkipWhitespace();
            if (_position < _text.Length)
            {
                throw new FormatException($"unexpected '{_text[_position]}' at position {_position + 1}");
            }
        }

        private double ParseAdditive()
        {
            var left = ParseMultiplicative();
            while (true)
            {
                SkipWhitespace();
                if (Match('+')) left += ParseMultiplicative();
                else if (Match('-')) left -= ParseMultiplicative();
                else return left;
            }
        }

        private double ParseMultiplicative()
        {
            var left = ParsePower();
            while (true)
            {
                SkipWhitespace();
                if (Match('*')) left *= ParsePower();
                else if (Match('/'))
                {
                    var divisor = ParsePower();
                    if (divisor == 0) throw new DivideByZeroException();
                    left /= divisor;
                }
                else if (Match('%'))
                {
                    var divisor = ParsePower();
                    if (divisor == 0) throw new DivideByZeroException();
                    left %= divisor;
                }
                else return left;
            }
        }

        /// <summary>Exponentiation binds tighter than multiplication and is right-associative.</summary>
        private double ParsePower()
        {
            var baseValue = ParseUnary();
            SkipWhitespace();
            return Match('^') ? Math.Pow(baseValue, ParsePower()) : baseValue;
        }

        private double ParseUnary()
        {
            SkipWhitespace();
            if (Match('-')) return -ParseUnary();
            if (Match('+')) return ParseUnary();
            return ParsePrimary();
        }

        private double ParsePrimary()
        {
            SkipWhitespace();
            if (_position >= _text.Length) throw new FormatException("the expression ended early");

            if (Match('('))
            {
                var value = ParseExpression();
                SkipWhitespace();
                if (!Match(')')) throw new FormatException("a '(' was never closed");
                return value;
            }

            var c = _text[_position];
            if (char.IsAsciiDigit(c) || c == '.') return ParseNumber();
            if (char.IsAsciiLetter(c)) return ParseIdentifier();

            throw new FormatException($"unexpected '{c}' at position {_position + 1}");
        }

        private double ParseNumber()
        {
            var start = _position;
            while (_position < _text.Length
                   && (char.IsAsciiDigit(_text[_position]) || _text[_position] == '.'))
            {
                _position++;
            }
            // Scientific notation, as in 1.5e-3.
            if (_position < _text.Length && (_text[_position] is 'e' or 'E'))
            {
                var save = _position;
                _position++;
                if (_position < _text.Length && (_text[_position] is '+' or '-')) _position++;
                if (_position < _text.Length && char.IsAsciiDigit(_text[_position]))
                {
                    while (_position < _text.Length && char.IsAsciiDigit(_text[_position])) _position++;
                }
                else
                {
                    _position = save; // not an exponent after all
                }
            }

            var slice = _text[start.._position];
            return double.TryParse(slice, NumberStyles.Float, CultureInfo.InvariantCulture, out var value)
                ? value
                : throw new FormatException($"'{slice}' is not a number");
        }

        private double ParseIdentifier()
        {
            var start = _position;
            while (_position < _text.Length && char.IsAsciiLetterOrDigit(_text[_position])) _position++;
            var name = _text[start.._position].ToLowerInvariant();

            switch (name)
            {
                case "pi": return Math.PI;
                case "e": return Math.E;
                case "tau": return Math.Tau;
            }

            SkipWhitespace();
            if (!Match('(')) throw new FormatException($"'{name}' is not a known constant");

            var arguments = new List<double>();
            SkipWhitespace();
            if (!Match(')'))
            {
                do
                {
                    arguments.Add(ParseExpression());
                    SkipWhitespace();
                }
                while (Match(','));

                if (!Match(')')) throw new FormatException($"'{name}(' was never closed");
            }

            return Apply(name, arguments);
        }

        private static double Apply(string name, List<double> args)
        {
            double One(string fn) => args.Count == 1
                ? args[0]
                : throw new FormatException($"{fn} takes one argument, got {args.Count}");

            double Two(string fn, int index) => args.Count == 2
                ? args[index]
                : throw new FormatException($"{fn} takes two arguments, got {args.Count}");

            return name switch
            {
                "sqrt" => Math.Sqrt(One("sqrt")),
                "abs" => Math.Abs(One("abs")),
                "floor" => Math.Floor(One("floor")),
                "ceil" or "ceiling" => Math.Ceiling(One("ceil")),
                "round" => args.Count switch
                {
                    1 => Math.Round(args[0], MidpointRounding.ToEven),
                    2 => Math.Round(args[0], (int)args[1], MidpointRounding.ToEven),
                    _ => throw new FormatException($"round takes one or two arguments, got {args.Count}"),
                },
                "ln" or "log" => Math.Log(One(name)),
                "log10" => Math.Log10(One("log10")),
                "log2" => Math.Log2(One("log2")),
                "exp" => Math.Exp(One("exp")),
                "sin" => Math.Sin(One("sin")),
                "cos" => Math.Cos(One("cos")),
                "tan" => Math.Tan(One("tan")),
                "pow" => Math.Pow(Two("pow", 0), Two("pow", 1)),
                "min" => Math.Min(Two("min", 0), Two("min", 1)),
                "max" => Math.Max(Two("max", 0), Two("max", 1)),
                _ => throw new FormatException($"'{name}' is not a known function"),
            };
        }

        private bool Match(char expected)
        {
            SkipWhitespace();
            if (_position < _text.Length && _text[_position] == expected)
            {
                _position++;
                return true;
            }
            return false;
        }

        private void SkipWhitespace()
        {
            while (_position < _text.Length && char.IsWhiteSpace(_text[_position])) _position++;
        }
    }
}
