using System;

namespace Conformance;

// Milestone 8.6: what library code needed first. Microsoft.Extensions.Hosting
// and Orleans check every argument with these guards, so a program using
// either stopped at its first call.
static partial class Program
{
    static void LibraryGuardChecks()
    {
        object present = "x";
        ArgumentNullException.ThrowIfNull(present);
        string caught = "none";
        object missing = null;
        try { ArgumentNullException.ThrowIfNull(missing); }
        catch (ArgumentNullException e) { caught = e.Message; }
        Check("ArgumentNullException.ThrowIfNull names the argument", caught == "Value cannot be null. (Parameter 'missing')");

        string empty = "";
        try { ArgumentException.ThrowIfNullOrEmpty(empty); caught = "none"; }
        catch (ArgumentException e) { caught = e.GetType().Name + ": " + e.Message; }
        Check("ThrowIfNullOrEmpty refuses an empty string", caught == "ArgumentException: The value cannot be an empty string. (Parameter 'empty')");

        string blank = "  ";
        ArgumentException.ThrowIfNullOrEmpty(blank);
        try { ArgumentException.ThrowIfNullOrWhiteSpace(blank); caught = "none"; }
        catch (ArgumentException e) { caught = e.Message; }
        Check("ThrowIfNullOrWhiteSpace refuses blanks", caught == "The value cannot be an empty string or composed entirely of whitespace. (Parameter 'blank')");

        string nothing = null;
        try { ArgumentException.ThrowIfNullOrWhiteSpace(nothing); caught = "none"; }
        catch (ArgumentNullException e) { caught = e.Message; }
        Check("and a null as ArgumentNullException", caught == "Value cannot be null. (Parameter 'nothing')");
    }
}
