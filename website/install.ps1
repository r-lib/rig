<#
.SYNOPSIS
    rig user-mode installer for Windows.

.DESCRIPTION
    Installs the `rig` binary (and gsudo.exe, plus shell completions) into
    your user profile, with no administrator privileges required.
    See https://github.com/r-lib/rig for details.

.EXAMPLE
    irm https://r-lib.github.io/rig/install.ps1 | iex

.EXAMPLE
    # With options, download first then run:
    irm https://r-lib.github.io/rig/install.ps1 -OutFile install.ps1
    .\install.ps1 -Prefix "$env:USERPROFILE\.local" -NoModifyPath

.PARAMETER Prefix
    Install prefix. Default: $env:USERPROFILE\.local
    (or $env:RIG_PREFIX if set).

.PARAMETER Version
    Version to install, e.g. 0.10.0. Default: latest
    (or $env:RIG_VERSION if set).

.PARAMETER NoModifyPath
    Do not add the bin directory to the user PATH.
#>
[CmdletBinding()]
param(
    [string] $Prefix       = $(if ($env:RIG_PREFIX) { $env:RIG_PREFIX } else { "$env:USERPROFILE\.local" }),
    [string] $Version      = $(if ($env:RIG_VERSION) { $env:RIG_VERSION } else { "latest" }),
    [switch] $NoModifyPath
)

$ErrorActionPreference = "Stop"
$Repo = "r-lib/rig"

# --- Detect architecture ------------------------------------------------
# Existing rig Windows assets use `x86_64` and `arm64` arch tokens.
switch ($env:PROCESSOR_ARCHITECTURE) {
    "AMD64" { $arch = "x86_64" }
    "ARM64" { $arch = "arm64" }
    "x86"   { $arch = "x86_64" }   # 32-bit shell on 64-bit Windows
    default { throw "Unsupported architecture: $($env:PROCESSOR_ARCHITECTURE)" }
}

# --- Work out the release asset URL -------------------------------------
# The moving `latest` release serves `...-latest.*` assets under the
# `latest` tag; versioned releases serve `...-X.Y.Z.*` under the `vX.Y.Z` tag.
if ($Version -eq "latest") {
    $tag = "latest"; $vtoken = "latest"
} else {
    $tag = "v$Version"; $vtoken = $Version
}

$asset = "rig-windows-$arch-$vtoken.zip"
$url   = "https://github.com/$Repo/releases/download/$tag/$asset"

# --- Download and extract -----------------------------------------------

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("rig-install-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $zip = Join-Path $tmp "rig.zip"
    Write-Host "Downloading $asset ..."
    Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing

    Write-Host "Installing into $Prefix ..."
    New-Item -ItemType Directory -Force -Path $Prefix | Out-Null
    Expand-Archive -Path $zip -DestinationPath $Prefix -Force
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

$bindir = Join-Path $Prefix "bin"
if (-not (Test-Path (Join-Path $bindir "rig.exe"))) {
    throw "Installation failed: rig.exe not found in $bindir"
}

# --- Optionally add the bin directory to the user PATH ------------------

if (-not $NoModifyPath) {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $parts = @()
    if ($userPath) { $parts = $userPath -split ";" }
    if ($parts -notcontains $bindir) {
        $newPath = if ($userPath) { "$userPath;$bindir" } else { $bindir }
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
        # Also update the current session so `rig` works right away.
        $env:Path = "$env:Path;$bindir"
        Write-Host "Added $bindir to your user PATH."
        Write-Host "Open a new terminal for it to take effect in other sessions."
    }
}

# --- Report next steps --------------------------------------------------

Write-Host ""
Write-Host "rig has been installed to $bindir\rig.exe"
Write-Host ""
Write-Host "To use rig in user mode (no admin needed), run:"
Write-Host "    rig system user-mode"
Write-Host "    rig add release"
Write-Host ""
Write-Host "For PowerShell tab-completion, dot-source the completion script"
Write-Host "from your profile (`$PROFILE):"
Write-Host "    . `"$Prefix\share\rig\_rig.ps1`""
