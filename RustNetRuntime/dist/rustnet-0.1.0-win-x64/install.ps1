<#
.SYNOPSIS
    Installs RustNetRuntime on Windows.

.DESCRIPTION
    Copies the toolchain and CodeGen into a prefix, puts the binaries on PATH,
    and adds a Start Menu shortcut. A per-user install needs no elevation and is
    the default; -System installs for everyone and requires an elevated shell.

.PARAMETER System
    Install for all users into %ProgramFiles%. Requires elevation.

.PARAMETER Prefix
    Install somewhere specific instead.

.PARAMETER Uninstall
    Remove a previous installation.

.EXAMPLE
    .\install.ps1
    .\install.ps1 -System
    .\install.ps1 -Prefix D:\Tools\RustNet
    .\install.ps1 -Uninstall
#>
[CmdletBinding()]
param(
    [switch]$System,
    [string]$Prefix,
    [switch]$Uninstall
)

$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

# ── Where ────────────────────────────────────────────────────────────────────
if ($Prefix) {
    $root = $Prefix
    $scope = 'User'
} elseif ($System) {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        Write-Error "-System needs an elevated shell. Re-run as administrator, or drop the flag for a per-user install."
        exit 1
    }
    $root = Join-Path $env:ProgramFiles 'RustNetRuntime'
    $scope = 'Machine'
} else {
    $root = Join-Path $env:LOCALAPPDATA 'RustNetRuntime'
    $scope = 'User'
}

$binDir = Join-Path $root 'bin'

# ── Uninstall ────────────────────────────────────────────────────────────────
if ($Uninstall) {
    Write-Host "Removing RustNetRuntime from $root"

    $path = [Environment]::GetEnvironmentVariable('Path', $scope)
    if ($path -and $path.Split(';') -contains $binDir) {
        $trimmed = ($path.Split(';') | Where-Object { $_ -ne $binDir }) -join ';'
        [Environment]::SetEnvironmentVariable('Path', $trimmed, $scope)
        Write-Host "  removed $binDir from PATH"
    }

    $shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'CodeGen.lnk'
    if (Test-Path $shortcut) { Remove-Item $shortcut -Force }

    if (Test-Path $root) { Remove-Item $root -Recurse -Force }
    Write-Host "Done. Your projects and settings were not touched."
    exit 0
}

# ── Preconditions ────────────────────────────────────────────────────────────
$sourceExe = Join-Path $here 'bin\rustnet.exe'
if (-not (Test-Path $sourceExe)) {
    Write-Error "bin\rustnet.exe is missing - run this from inside an unpacked package."
    exit 1
}

Write-Host "Installing RustNetRuntime into $root"
Write-Host ""

# ── Files ────────────────────────────────────────────────────────────────────
New-Item -ItemType Directory -Force -Path $binDir | Out-Null
Copy-Item $sourceExe (Join-Path $binDir 'rustnet.exe') -Force
Write-Host "  toolchain    $binDir\rustnet.exe"

$codegenSource = Join-Path $here 'codegen'
if (Test-Path $codegenSource) {
    $codegenTarget = Join-Path $root 'codegen'
    if (Test-Path $codegenTarget) { Remove-Item $codegenTarget -Recurse -Force }
    Copy-Item $codegenSource $codegenTarget -Recurse -Force

    # A shim rather than a copy: CodeGen resolves app.config next to its own
    # assembly, so it has to run from its install directory.
    $shim = Join-Path $binDir 'codegen.cmd'
    "@echo off`r`nstart """" ""$codegenTarget\CodeGen.exe"" %*" | Set-Content -Path $shim -Encoding ASCII
    Write-Host "  IDE          $codegenTarget\CodeGen.exe"
}

foreach ($item in @('docs', 'samples')) {
    $source = Join-Path $here $item
    if (Test-Path $source) {
        $target = Join-Path $root $item
        if (Test-Path $target) { Remove-Item $target -Recurse -Force }
        Copy-Item $source $target -Recurse -Force
        Write-Host "  $item".PadRight(15) + "$target"
    }
}
foreach ($file in @('README.md', 'README.id.md', 'LICENSE', 'VERSION')) {
    $source = Join-Path $here $file
    if (Test-Path $source) { Copy-Item $source (Join-Path $root $file) -Force }
}

# ── PATH ─────────────────────────────────────────────────────────────────────
$path = [Environment]::GetEnvironmentVariable('Path', $scope)
if (-not $path) { $path = '' }
if ($path.Split(';') -notcontains $binDir) {
    $updated = if ($path.TrimEnd(';')) { "$($path.TrimEnd(';'));$binDir" } else { $binDir }
    [Environment]::SetEnvironmentVariable('Path', $updated, $scope)
    Write-Host ""
    Write-Host "  Added $binDir to your $scope PATH."
    Write-Host "  Open a new terminal for it to take effect."
}

# ── Start Menu ───────────────────────────────────────────────────────────────
$codegenExe = Join-Path $root 'codegen\CodeGen.exe'
if (Test-Path $codegenExe) {
    $programs = [Environment]::GetFolderPath('Programs')
    $shortcutPath = Join-Path $programs 'CodeGen.lnk'
    try {
        $shell = New-Object -ComObject WScript.Shell
        $shortcut = $shell.CreateShortcut($shortcutPath)
        $shortcut.TargetPath = $codegenExe
        $shortcut.WorkingDirectory = (Join-Path $root 'codegen')
        $shortcut.Description = 'CodeGen - write C# and run it on RustCLR'
        $shortcut.Save()
        Write-Host "  Start Menu   $shortcutPath"
    } catch {
        Write-Host "  note: could not create the Start Menu shortcut: $($_.Exception.Message)"
    }
}

# ── The one dependency we do not ship ────────────────────────────────────────
Write-Host ""
if (-not (Get-Command dotnet -ErrorAction SilentlyContinue)) {
    Write-Host "  Note: the .NET SDK was not found."
    Write-Host "        RustCLR runs IL; it does not compile C#. Install the SDK from"
    Write-Host "        https://dotnet.microsoft.com/download to build projects."
    Write-Host ""
}

Write-Host "Installed. Try it in a new terminal:"
Write-Host ""
Write-Host "    rustnet capabilities"
Write-Host "    codegen"
Write-Host ""
Write-Host "Built by Gravicode Studios, led by Kang Fadhil."
