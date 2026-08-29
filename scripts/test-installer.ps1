$ErrorActionPreference = "Stop"
Set-StrictMode -Version 2.0
$repo = Split-Path -Parent $PSScriptRoot
$manifest = Get-Content -LiteralPath (Join-Path $repo "Cargo.toml") -Raw
$version = [regex]::Match($manifest, '(?m)^version = "([0-9]+\.[0-9]+\.[0-9]+)"\r?$').Groups[1].Value
if (-not $version) { throw "could not read package version" }
$temporary = Join-Path ([System.IO.Path]::GetTempPath()) ("open-agent-view-installer-test-" + [guid]::NewGuid())
try {
    $release = Join-Path $temporary "releases\v$version"
    $install = Join-Path $temporary "installed"
    New-Item -ItemType Directory -Path $release -Force | Out-Null
    & (Join-Path $repo "scripts\package-release.ps1") -Binary (Join-Path $repo "target\x86_64-pc-windows-msvc\release\open-agent-view.exe") -DistDir $release | Out-Null
    & (Join-Path $repo "install.ps1") -Version $version -InstallDir $install -ReleaseBaseUrl (Join-Path $temporary "releases") -SkipPathUpdate
    $canonical = Join-Path $install "open-agent-view.exe"
    $alias = Join-Path $install "oav.exe"
    $legacyAlias = Join-Path $install "opav.exe"
    if (-not (Test-Path -LiteralPath $canonical)) { throw "canonical executable was not installed" }
    if (-not (Test-Path -LiteralPath $alias)) { throw "oav shorthand was not installed" }
    if (-not (Test-Path -LiteralPath $legacyAlias)) { throw "legacy opav compatibility alias was not installed" }
    if ((& $canonical --version | Out-String).Trim() -ne "open-agent-view $version") { throw "canonical executable version mismatch" }
    if ((& $alias --version | Out-String).Trim() -ne "open-agent-view $version") { throw "oav shorthand version mismatch" }
    if ((& $legacyAlias --version | Out-String).Trim() -ne "open-agent-view $version") { throw "legacy opav compatibility alias version mismatch" }

    $collisionInstall = Join-Path $temporary "collision"
    New-Item -ItemType Directory -Path $collisionInstall -Force | Out-Null
    $collisionOav = Join-Path $collisionInstall "oav.exe"
    $collisionLegacy = Join-Path $collisionInstall "opav.exe"
    Copy-Item -LiteralPath $env:ComSpec -Destination $collisionOav
    Copy-Item -LiteralPath $env:ComSpec -Destination $collisionLegacy
    $collisionOavHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $collisionOav).Hash
    $collisionLegacyHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $collisionLegacy).Hash
    & (Join-Path $repo "install.ps1") -Version $version -InstallDir $collisionInstall -ReleaseBaseUrl (Join-Path $temporary "releases") -SkipPathUpdate
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $collisionOav).Hash -ne $collisionOavHash) { throw "installer replaced an unrelated oav.exe" }
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $collisionLegacy).Hash -ne $collisionLegacyHash) { throw "installer replaced an unrelated legacy opav.exe" }

    $waiter = Start-Process powershell.exe -ArgumentList "-NoProfile", "-Command", "Start-Sleep -Seconds 2" -PassThru
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    & (Join-Path $repo "install.ps1") -Version $version -InstallDir $install -ReleaseBaseUrl (Join-Path $temporary "releases") -WaitForProcessId $waiter.Id -PreviousVersion "0.0.0" -SkipPathUpdate
    $timer.Stop()
    if ($timer.Elapsed.TotalSeconds -lt 1.5) { throw "installer did not wait for the running process before replacement" }
    if ((& $canonical --version | Out-String).Trim() -ne "open-agent-view $version") { throw "waited replacement produced the wrong executable" }

    $checksum = Get-ChildItem -LiteralPath $release -Filter '*.sha256' | Select-Object -First 1
    $original = [System.IO.File]::ReadAllText($checksum.FullName)
    if ($original.Contains("`r")) { throw "release checksum must use portable LF line endings" }
    if (-not $original.EndsWith("`n")) { throw "release checksum must end with a newline" }
    [System.IO.File]::WriteAllText($checksum.FullName, (('0' * 64) + "  broken.zip`n"), [System.Text.Encoding]::ASCII)
    $failed = $false
    try {
        & (Join-Path $repo "install.ps1") -Version $version -InstallDir $install -ReleaseBaseUrl (Join-Path $temporary "releases") -SkipPathUpdate
    } catch {
        $failed = $_.Exception.Message -match 'checksum verification failed'
    }
    if (-not $failed) { throw "installer accepted an invalid checksum" }
    [System.IO.File]::WriteAllText($checksum.FullName, $original, [System.Text.Encoding]::ASCII)
    if ((& $canonical --version | Out-String).Trim() -ne "open-agent-view $version") { throw "failed install replaced the working executable" }
    Write-Host "Windows installer tests passed"
} finally {
    Remove-Item -LiteralPath $temporary -Recurse -Force -ErrorAction SilentlyContinue
}
