<#
.SYNOPSIS
    Installer for seogeo — the engine behind the SEO + GEO agent skills.

.DESCRIPTION
    Detects the platform, downloads the matching release archive, verifies its
    SHA-256 against the published SHA256SUMS, installs the binary to a
    per-user directory, and installs the bundled skills into every agent tool
    it finds.

    SPDX-License-Identifier: MIT

.PARAMETER Version
    Tag to install, e.g. v0.1.0. Defaults to the latest release.

.PARAMETER BinDir
    Install directory. Defaults to %LOCALAPPDATA%\Programs\seogeo.

.PARAMETER Target
    How to install the skills:
      auto (default)  npx skills if Node is present, otherwise the built-in installer
      npx             npx skills — one copy, symlinked into 75+ agents
      all             write the five known agent paths directly
      claude | codex | gemini | opencode | agents   a single tool
      none            skip skills entirely

.PARAMETER NoSkills
    Install only the binary.

.EXAMPLE
    irm https://raw.githubusercontent.com/asale-ai/seo-geo-skill/main/install.ps1 | iex

.EXAMPLE
    .\install.ps1 -Version v0.1.0 -Target claude
#>
[CmdletBinding()]
param(
    [string]$Version = $env:SEOGEO_VERSION,
    [string]$BinDir  = $env:SEOGEO_BIN_DIR,
    [string]$Target  = $(if ($env:SEOGEO_TARGET) { $env:SEOGEO_TARGET } else { 'auto' }),
    [switch]$NoSkills
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Repo    = 'asale-ai/seo-geo-skill'
$BinName = 'seogeo'

function Write-Step { param([string]$Message) Write-Host "==> $Message" -ForegroundColor Cyan }
function Write-Info { param([string]$Message) Write-Host "    $Message" }
function Write-Warn { param([string]$Message) Write-Host "warning: $Message" -ForegroundColor Yellow }
function Stop-WithError {
    param([string]$Message)
    Write-Host "error: $Message" -ForegroundColor Red
    exit 1
}

function Get-Target {
    # PROCESSOR_ARCHITECTURE reports the *process* architecture, which is x86
    # inside a 32-bit shell on a 64-bit machine. RuntimeInformation reports the
    # OS, which is what we actually need.
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    switch ($arch) {
        'X64'   { return 'x86_64-pc-windows-msvc' }
        'Arm64' {
            # No native arm64 build yet; x64 runs under emulation on Windows 11
            # on ARM, so prefer a working install over a hard failure.
            Write-Warn 'No native ARM64 build yet — installing the x64 build, which Windows emulates.'
            return 'x86_64-pc-windows-msvc'
        }
        default {
            Stop-WithError @"
Unsupported architecture: $arch
seogeo ships an x86_64 Windows build. Build from source instead:
    cargo install --git https://github.com/$Repo
"@
        }
    }
}

function Get-LatestVersion {
    Write-Step 'Resolving the latest release'
    $api = "https://api.github.com/repos/$Repo/releases/latest"
    try {
        $release = Invoke-RestMethod -Uri $api -Headers @{ 'User-Agent' = 'seogeo-installer' } -TimeoutSec 30
    } catch {
        Stop-WithError @"
Could not reach the GitHub API: $($_.Exception.Message)
Set -Version vX.Y.Z to install a specific release.
"@
    }
    if (-not $release.tag_name) {
        Stop-WithError 'The GitHub API returned no tag_name. Set -Version vX.Y.Z explicitly.'
    }
    return $release.tag_name
}

# TLS 1.2 is not the default on older PowerShell hosts, and GitHub requires it.
try {
    [Net.ServicePointManager]::SecurityProtocol =
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch { }

if (-not $BinDir) {
    $BinDir = Join-Path $env:LOCALAPPDATA 'Programs\seogeo'
}
if (-not $Version) {
    $Version = Get-LatestVersion
}

$targetTriple = Get-Target
$numVersion   = $Version.TrimStart('v')
$asset        = "$BinName-$numVersion-$targetTriple.zip"
$base         = "https://github.com/$Repo/releases/download/$Version"

Write-Info "version:  $Version"
Write-Info "platform: $targetTriple"
Write-Info "install:  $BinDir"
Write-Host ''

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "seogeo-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $tmp -Force | Out-Null

try {
    Write-Step "Downloading $asset"
    $archive = Join-Path $tmp $asset
    try {
        Invoke-WebRequest -Uri "$base/$asset" -OutFile $archive -UseBasicParsing -TimeoutSec 300
    } catch {
        Stop-WithError @"
Download failed: $base/$asset
$($_.Exception.Message)

The release may not include a build for $targetTriple.
See https://github.com/$Repo/releases/tag/$Version
"@
    }

    Write-Step 'Verifying checksum'
    $sumsFile = Join-Path $tmp 'SHA256SUMS'
    $verified = $false
    try {
        Invoke-WebRequest -Uri "$base/SHA256SUMS" -OutFile $sumsFile -UseBasicParsing -TimeoutSec 60
        $line = Get-Content $sumsFile | Where-Object { $_ -match [regex]::Escape($asset) + '\s*$' } | Select-Object -First 1
        if ($line) {
            $expected = ($line -split '\s+')[0].ToLower()
            $actual   = (Get-FileHash -Path $archive -Algorithm SHA256).Hash.ToLower()
            if ($expected -ne $actual) {
                Stop-WithError @"
Checksum mismatch for $asset
  expected: $expected
  actual:   $actual

Nothing was installed. This could mean a corrupted download or a tampered
artifact — re-run, and report it if it persists.
"@
            }
            Write-Info "ok: $actual"
            $verified = $true
        }
    } catch {
        Write-Warn "Could not download SHA256SUMS: $($_.Exception.Message)"
    }
    if (-not $verified) {
        Write-Warn "$asset was not verified against a published checksum."
    }

    Write-Step 'Extracting'
    Expand-Archive -Path $archive -DestinationPath $tmp -Force
    $exe = Get-ChildItem -Path $tmp -Filter "$BinName.exe" -Recurse |
           Select-Object -First 1
    if (-not $exe) {
        Stop-WithError "The archive did not contain $BinName.exe"
    }

    Write-Step "Installing to $BinDir"
    New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
    $dest = Join-Path $BinDir "$BinName.exe"
    try {
        Copy-Item -Path $exe.FullName -Destination $dest -Force
    } catch {
        Stop-WithError @"
Cannot write to $BinDir
$($_.Exception.Message)

If seogeo is currently running, close it and re-run. Otherwise pass
-BinDir with a writable location.
"@
    }

    $installed = & $dest --version 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $installed) {
        Stop-WithError @"
The installed binary did not run.
This usually means an architecture mismatch — detected $targetTriple.
"@
    }
    Write-Host "    installed $installed" -ForegroundColor Green

    if (-not $NoSkills -and $Target -ne 'none') {
        Write-Host ''
        Write-Step 'Installing skills'

        # npx skills keeps one copy and symlinks it into 75+ agents, against
        # the five this binary writes itself. Prefer it when Node is present,
        # but never require it.
        $resolved = $Target
        if ($resolved -eq 'auto') {
            if (Get-Command npx -ErrorAction SilentlyContinue) {
                $resolved = 'npx'
            } else {
                $resolved = 'all'
                Write-Info 'Node not found; using the built-in installer (5 agent tools)'
            }
        }

        & $dest install --target $resolved
        if ($LASTEXITCODE -ne 0) {
            if ($resolved -eq 'npx') {
                Write-Warn 'npx skills failed; falling back to the built-in installer'
                & $dest install --target all
            }
            if ($LASTEXITCODE -ne 0) {
                Write-Warn "Skill installation failed; run '$BinName install --target all' by hand."
            }
        }
    }

    Write-Host ''
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $onPath = ($userPath -split ';' | Where-Object { $_.TrimEnd('\') -ieq $BinDir.TrimEnd('\') }).Count -gt 0

    if ($onPath) {
        Write-Host 'Done.' -ForegroundColor Green
        Write-Info "Try: $BinName install --list"
    } else {
        Write-Step 'Adding to your user PATH'
        try {
            $newPath = if ([string]::IsNullOrEmpty($userPath)) { $BinDir } else { "$userPath;$BinDir" }
            [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
            $env:Path = "$env:Path;$BinDir"
            Write-Host 'Done.' -ForegroundColor Green
            Write-Info 'PATH updated. Open a new terminal for it to take effect everywhere.'
        } catch {
            Write-Host 'Done.' -ForegroundColor Green
            Write-Host ''
            Write-Warn "$BinDir is not on your PATH and it could not be added automatically."
            Write-Info 'The skills call seogeo by name, so add it manually:'
            Write-Info "  [Environment]::SetEnvironmentVariable('Path', `"`$env:Path;$BinDir`", 'User')"
        }
    }
} finally {
    Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
