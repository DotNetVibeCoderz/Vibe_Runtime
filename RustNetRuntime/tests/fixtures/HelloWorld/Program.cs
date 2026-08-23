using System;

namespace HelloWorld;

public static class Program
{
    public static int Add(int a, int b) => a + b;

    public static int Factorial(int n)
    {
        int acc = 1;
        for (int i = 2; i <= n; i++) acc *= i;
        return acc;
    }

    public static void Main()
    {
        Console.WriteLine("Hello from RustCLR");
        Console.WriteLine(Add(2, 40));
        Console.WriteLine(Factorial(5));
    }
}
