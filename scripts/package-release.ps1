[CmdletBinding()]
param(
    [string]$Target = "x86_64-pc-windows-msvc",
    [string]$Binary,
    [string]$DistDir
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version 2.0
$repo = Split-Path -Parent $PSScriptRoot
if ($Target -ne "x86_64-pc-windows-msvc") { throw "supported target: x86_64-pc-windows-msvc" }
if (-not $DistDir) { $DistDir = Join-Path $repo "dist" }
if (-not $Binary) { $Binary = Join-Path $repo "target\$Target\release\open-agent-view.exe" }
if (-not (Test-Path -LiteralPath $Binary -PathType Leaf)) { throw "release binary is missing: $Binary" }

$manifest = Get-Content -LiteralPath (Join-Path $repo "Cargo.toml") -Raw
$match = [regex]::Match($manifest, '(?m)^version = "([0-9]+\.[0-9]+\.[0-9]+)"\r?$')
if (-not $match.Success) { throw "could not read release version from Cargo.toml" }
$version = $match.Groups[1].Value
$reported = (& $Binary --version | Out-String).Trim()
if ($reported -ne "open-agent-view $version") { throw "binary reports an unexpected version: $reported" }

$stem = "open-agent-view-$version-$Target"
$temporary = Join-Path ([System.IO.Path]::GetTempPath()) ("open-agent-view-package-" + [guid]::NewGuid())
$stage = Join-Path $temporary $stem
$archive = Join-Path $DistDir "$stem.zip"
try {
    New-Item -ItemType Directory -Path $stage -Force | Out-Null
    New-Item -ItemType Directory -Path $DistDir -Force | Out-Null
    Copy-Item -LiteralPath $Binary -Destination (Join-Path $stage "open-agent-view.exe")
    Copy-Item -LiteralPath (Join-Path $repo "LICENSE") -Destination $stage
    Copy-Item -LiteralPath (Join-Path $repo "README.md") -Destination $stage
    Compress-Archive -Path $stage -DestinationPath $archive -Force
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $archive).Hash.ToLowerInvariant()
    $checksumLine = "$hash  $([System.IO.Path]::GetFileName($archive))`n"
    [System.IO.File]::WriteAllText("$archive.sha256", $checksumLine, [System.Text.Encoding]::ASCII)
    Write-Output $archive
    Write-Output "$archive.sha256"
} finally {
    Remove-Item -LiteralPath $temporary -Recurse -Force -ErrorAction SilentlyContinue
}
