#Requires -Version 5.1
<#
.SYNOPSIS
    pst installer for Windows.
.EXAMPLE
    irm https://raw.githubusercontent.com/quangdang46/prompt_storage/main/install.ps1 | iex
#>
[CmdletBinding()]
param(
    [string]$Dest = "$env:USERPROFILE\.local\bin",
    [string]$Version = "",
    [switch]$EasyMode,
    [switch]$Verify,
    [switch]$FromSource,
    [switch]$Uninstall,
    [switch]$Quiet
)

$ErrorActionPreference = "Stop"
$BinaryName = "pst"
$Owner = "quangdang46"
$Repo = "prompt_storage"
$MaxRetries = 3

function Write-Info { if (-not $Quiet) { Write-Host "[$BinaryName] $args" -ForegroundColor Gray } }
function Write-Warn2 { Write-Host "[$BinaryName] WARN: $args" -ForegroundColor Yellow }
function Die { Write-Host "ERROR: $args" -ForegroundColor Red; exit 1 }

if ($Uninstall) {
    Remove-Item -Force -ErrorAction SilentlyContinue "$Dest\$BinaryName.exe"
    # Remove PATH entry added by easy-mode
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($userPath -like "*$Dest*") {
        $newPath = ($userPath -split ";" | Where-Object { $_ -ne $Dest }) -join ";"
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    }
    Write-Host "✓ $BinaryName uninstalled"
    exit 0
}

# Platform: windows_x86_64 — split into exactly 2 parts (arch contains _64)
$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
    "AMD64" { "x86_64" }
    "ARM64" { "aarch64" }
    default { Die "Unsupported architecture: $env:PROCESSOR_ARCHITECTURE" }
}
$platform = "windows_$arch"

# Version resolution
if (-not $Version) {
    try {
        $rel = Invoke-RestMethod -Uri "https://api.github.com/repos/$Owner/$Repo/releases/latest" `
            -Headers @{ Accept = "application/vnd.github.v3+json" } `
            -TimeoutSec 30
        $Version = $rel.tag_name
    } catch {
        Die "Could not resolve latest version: $_"
    }
}
Write-Info "Latest release: $Version"

# Download with retry
$archive = "$BinaryName-$platform.zip"
$url = "https://github.com/$Owner/$Repo/releases/download/$Version/$archive"
$tmp = New-Item -ItemType Directory -Force -Path (Join-Path $env:TEMP "$BinaryName-install")
$archivePath = Join-Path $tmp $archive
$downloaded = $false

for ($i = 1; $i -le $MaxRetries; $i++) {
    try {
        Write-Info "Downloading $url"
        Invoke-WebRequest -Uri $url -OutFile $archivePath -TimeoutSec 120
        $downloaded = $true
        break
    } catch {
        if ($i -lt $MaxRetries) { Write-Warn2 "Retry $i..."; Start-Sleep 3 }
    }
}
if (-not $downloaded) {
    Write-Warn2 "Download failed — falling back to source build requires cargo."
    if (Get-Command cargo -ErrorAction SilentlyContinue) {
        $src = Join-Path $tmp "src"
        git clone --depth 1 "https://github.com/$Owner/$Repo.git" $src
        Push-Location $src
        $env:CARGO_TARGET_DIR = Join-Path $tmp "target"
        cargo build --release -p pst
        Pop-Location
        New-Item -ItemType Directory -Force -Path $Dest | Out-Null
        Copy-Item (Join-Path $tmp "target\release\$BinaryName.exe") "$Dest\$BinaryName.exe" -Force
    } else {
        Die "cargo not found — install Rust: https://rustup.rs"
    }
} else {
    # Checksum verification
    $checksumUrl = "$url.sha256"
    $checksumPath = Join-Path $tmp "checksum.sha256"
    try {
        Invoke-WebRequest -Uri $checksumUrl -OutFile $checksumPath -TimeoutSec 30
        $expected = (Get-Content $checksumPath -Raw).Split(" ")[0].Trim()
        $actual = (Get-FileHash $archivePath -Algorithm SHA256).Hash.ToLower()
        if ($expected -ne $actual) { Die "Checksum mismatch" }
        Write-Info "Checksum verified"
    } catch { Write-Warn2 "Checksum sidecar unavailable — skipping verification" }

    Extract-Archive-Safe $archivePath $tmp.FullName
    $bin = Get-ChildItem -Path $tmp.FullName -Recurse -Filter "$BinaryName.exe" |
        Select-Object -First 1
    if (-not $bin) { Die "Binary not found after extract" }

    New-Item -ItemType Directory -Force -Path $Dest | Out-Null
    $destExe = "$Dest\$BinaryName.exe"
    Copy-Item $bin.FullName $destExe -Force
}

function Extract-Archive-Safe([string]$Zip, [string]$OutDir) {
    Expand-Archive -Path $Zip -DestinationPath $OutDir -Force
}

# PATH handling
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (($userPath -split ";") -notcontains $Dest) {
    if ($EasyMode) {
        [Environment]::SetEnvironmentVariable("Path", "$Dest;$userPath", "User")
        Write-Warn2 "PATH updated — restart your terminal"
    } else {
        Write-Warn2 "Add to PATH: $Dest"
    }
}

if ($Verify) {
    & "$Dest\$BinaryName.exe" --version
    if ($LASTEXITCODE -ne 0) { Die "Installed binary failed to run" }
}

Write-Host ""
Write-Host "✓ $BinaryName installed → $Dest\$BinaryName.exe" -ForegroundColor Green
Write-Host ""
Write-Host "  Quick start:"
Write-Host "    $BinaryName --help"
Write-Host "    $BinaryName new my-prompt --from -"
Write-Host "    $BinaryName my-prompt"
