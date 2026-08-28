[CmdletBinding()]
param(
    [string]$Version = $env:OAV_VERSION,
    [string]$InstallDir = $env:OAV_INSTALL_DIR,
    [string]$Repo = $(if ($env:OAV_REPO) { $env:OAV_REPO } else { "xhluca/open-agent-view" }),
    [string]$ReleaseBaseUrl = $env:OAV_RELEASE_BASE_URL,
    [int]$WaitForProcessId = 0,
    [string]$PreviousVersion = "",
    [switch]$CleanupStaging,
    [switch]$SkipPathUpdate
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version 2.0

function Write-OavMessage([string]$Message) {
    Write-Host "open-agent-view: $Message"
}

function Copy-OrDownload([string]$Source, [string]$Destination) {
    if (Test-Path -LiteralPath $Source) {
        Copy-Item -LiteralPath $Source -Destination $Destination -Force
    } else {
        Invoke-WebRequest -UseBasicParsing -Uri $Source -OutFile $Destination
    }
}

if ($WaitForProcessId -gt 0) {
    $running = Get-Process -Id $WaitForProcessId -ErrorAction SilentlyContinue
    if ($running) {
        Write-OavMessage "waiting for the running Open Agent View process to exit"
        $running.WaitForExit()
    }
}

if (-not $Version) { $Version = "latest" }
if (-not $InstallDir) {
    $base = if ($env:LOCALAPPDATA) { $env:LOCALAPPDATA } else { Join-Path $env:USERPROFILE "AppData\Local" }
    $InstallDir = Join-Path $base "Programs\OpenAgentView\bin"
}
if ($Repo -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') {
    throw "repository must have the form OWNER/REPO"
}

$architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
switch ($architecture) {
    "X64" { $target = "x86_64-pc-windows-msvc" }
    default { throw "no native Windows release is available for $architecture yet; use Windows x64 or WSL 2" }
}

if ($Version -eq "latest") {
    $headers = @{ "Accept" = "application/vnd.github+json" }
    if ($env:GH_TOKEN) { $headers["Authorization"] = "Bearer $($env:GH_TOKEN)" }
    $release = Invoke-RestMethod -Headers $headers -Uri "https://api.github.com/repos/$Repo/releases/latest"
    $tag = [string]$release.tag_name
} else {
    $tag = "v$($Version.TrimStart('v'))"
}
if ($tag -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+$') {
    throw "release version must have the form MAJOR.MINOR.PATCH (received: $tag)"
}
$resolvedVersion = $tag.Substring(1)
$stem = "open-agent-view-$resolvedVersion-$target"
$archive = "$stem.zip"
$checksum = "$archive.sha256"
$temporary = Join-Path ([System.IO.Path]::GetTempPath()) ("open-agent-view-install-" + [guid]::NewGuid())
$staged = Join-Path $InstallDir (".open-agent-view.install." + $PID + ".exe")

try {
    New-Item -ItemType Directory -Path $temporary -Force | Out-Null
    if ($ReleaseBaseUrl) {
        $base = $ReleaseBaseUrl.TrimEnd('/', '\') + "\$tag"
        if ($ReleaseBaseUrl -match '^https?://') { $base = $ReleaseBaseUrl.TrimEnd('/') + "/$tag" }
    } else {
        $base = "https://github.com/$Repo/releases/download/$tag"
    }
    Write-OavMessage "downloading $tag for $target"
    Copy-OrDownload "$base/$archive" (Join-Path $temporary $archive)
    Copy-OrDownload "$base/$checksum" (Join-Path $temporary $checksum)

    $expected = ((Get-Content -LiteralPath (Join-Path $temporary $checksum) -TotalCount 1) -split '\s+')[0]
    if ($expected -notmatch '^[0-9a-fA-F]{64}$') { throw "release checksum file is malformed" }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $temporary $archive)).Hash
    if ($actual -ne $expected) { throw "release checksum verification failed" }

    Expand-Archive -LiteralPath (Join-Path $temporary $archive) -DestinationPath $temporary -Force
    $binary = Join-Path (Join-Path $temporary $stem) "open-agent-view.exe"
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
        throw "release archive does not contain $stem/open-agent-view.exe"
    }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item -LiteralPath $binary -Destination $staged -Force
    $reported = (& $staged --version | Out-String).Trim()
    if ($reported -ne "open-agent-view $resolvedVersion") {
        throw "downloaded binary reported an unexpected version: $reported"
    }
    Move-Item -LiteralPath $staged -Destination (Join-Path $InstallDir "open-agent-view.exe") -Force
    Copy-Item -LiteralPath (Join-Path $InstallDir "open-agent-view.exe") -Destination (Join-Path $InstallDir "opav.exe") -Force

    if (-not $SkipPathUpdate) {
        $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
        $parts = @($userPath -split ';' | Where-Object { $_ })
        if ($parts -notcontains $InstallDir) {
            [Environment]::SetEnvironmentVariable("Path", (($parts + $InstallDir) -join ';'), "User")
        }
        if (($env:Path -split ';') -notcontains $InstallDir) { $env:Path = "$InstallDir;$env:Path" }
    }
    Write-OavMessage "installed open-agent-view $resolvedVersion to $InstallDir\open-agent-view.exe"
    Write-OavMessage "installed shorthand: opav"
    Write-OavMessage "open a new terminal, then run: open-agent-view (or opav)"
    if ($PreviousVersion) {
        if ($PreviousVersion -eq $resolvedVersion) {
            Write-OavMessage "Open Agent View is already up to date at $resolvedVersion"
        } else {
            Write-OavMessage "updated Open Agent View from $PreviousVersion to $resolvedVersion"
        }
    }
} finally {
    Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $temporary -Recurse -Force -ErrorAction SilentlyContinue
    if ($CleanupStaging -and $PSCommandPath) {
        $stagingDirectory = Split-Path -Parent $PSCommandPath
        $stagingParent = Split-Path -Parent $stagingDirectory
        $temporaryRoot = [System.IO.Path]::GetTempPath().TrimEnd('\', '/')
        $stagingName = Split-Path -Leaf $stagingDirectory
        if ($stagingParent.TrimEnd('\', '/') -eq $temporaryRoot -and $stagingName -match '^open-agent-view-update-[0-9]+-[0-9]+$') {
            Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath $stagingDirectory -Force -ErrorAction SilentlyContinue
        }
    }
}
