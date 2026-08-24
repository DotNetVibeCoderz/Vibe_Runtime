namespace __NAME__;

/// <summary>
/// Expression calculator demonstrating pure managed computation on the
/// RustNet runtime: a recursive-descent parser with +,-,*,/ and parens.
/// </summary>
public static class Program
{
    private static string _input = "";
    private static int _pos;

    public static void Main()
    {
        Console.WriteLine("__NAME__ calculator");
        Evaluate("1+2*3");
        Evaluate("(1+2)*3");
        Evaluate("10/4");
        Evaluate("2*(3+4)-5");
        Evaluate("100/(2+3)/5");
    }

    private static void Evaluate(string expr)
    {
        _input = expr;
        _pos = 0;
        double result = ParseExpression();
        Console.WriteLine(string.Concat(expr, " = ", result.ToString()));
    }

    private static double ParseExpression()
    {
        double value = ParseTerm();
        while (_pos < _input.Length)
        {
            char op = _input[_pos];
            if (op != '+' && op != '-')
            {
                break;
            }
            _pos = _pos + 1;
            double rhs = ParseTerm();
            if (op == '+')
            {
                value = value + rhs;
            }
            else
            {
                value = value - rhs;
            }
        }
        return value;
    }

    private static double ParseTerm()
    {
        double value = ParseFactor();
        while (_pos < _input.Length)
        {
            char op = _input[_pos];
            if (op != '*' && op != '/')
            {
                break;
            }
            _pos = _pos + 1;
            double rhs = ParseFactor();
            if (op == '*')
            {
                value = value * rhs;
            }
            else
            {
                value = value / rhs;
            }
        }
        return value;
    }

    private static double ParseFactor()
    {
        if (_pos < _input.Length && _input[_pos] == '(')
        {
            _pos = _pos + 1; // consume (
            double inner = ParseExpression();
            _pos = _pos + 1; // consume )
            return inner;
        }
        double number = 0.0;
        while (_pos < _input.Length && _input[_pos] >= '0' && _input[_pos] <= '9')
        {
            number = number * 10.0 + (_input[_pos] - '0');
            _pos = _pos + 1;
        }
        return number;
    }
}
