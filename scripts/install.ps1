# Herdr install-time build for Windows. Prefer the checksum-verified release
# binary only when it was built from this exact source revision; otherwise
# build the checked-out source with Cargo.
# Compatible with Windows PowerShell 5.1.
$ErrorActionPreference = 'Stop'

$Name = 'dagr'
$Repo = 'aemrebarut/herdr-dagr'
$Root = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if ($Root.StartsWith('\\?\')) { $Root = $Root.Substring(4) }
$BinDir = if ($env:DAGR_INSTALL_BIN_DIR) { $env:DAGR_INSTALL_BIN_DIR } else { Join-Path $Root 'bin' }
$Manifest = Join-Path $Root 'herdr-plugin.toml'
$VersionMatch = Select-String -Path $Manifest -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $VersionMatch) {
    [Console]::Error.WriteLine("${Name}: cannot read the plugin version")
    exit 1
}
$Version = $VersionMatch.Matches[0].Groups[1].Value
$Tag = "v$Version"
$script:TmpDir = $null

function Clear-DagrTemp {
    if ($script:TmpDir -and (Test-Path -LiteralPath $script:TmpDir)) {
        Remove-Item -LiteralPath $script:TmpDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Invoke-DagrFallback {
    param([string]$Reason)
    $Cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if ($Cargo) {
        [Console]::Error.WriteLine("${Name}: $Reason; building this source with Cargo")
        Clear-DagrTemp
        & (Join-Path $PSScriptRoot 'build.ps1')
        exit $LASTEXITCODE
    }
    [Console]::Error.WriteLine("${Name}: $Reason, and Cargo is not installed")
    [Console]::Error.WriteLine("${Name}: install the released revision, or install Rust to build this source")
    Clear-DagrTemp
    exit 1
}

function Invoke-DagrDownload {
    param([string]$Url, [string]$Destination)
    try {
        if ($Url.StartsWith('file://', [System.StringComparison]::OrdinalIgnoreCase)) {
            $Uri = New-Object System.Uri($Url)
            Copy-Item -LiteralPath $Uri.LocalPath -Destination $Destination -Force
        } else {
            try {
                [Net.ServicePointManager]::SecurityProtocol =
                    [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
            } catch {}
            Invoke-WebRequest -Uri $Url -OutFile $Destination -UseBasicParsing -ErrorAction Stop
        }
        return $true
    } catch {
        return $false
    }
}

$Arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
$Target = $null
if ($Arch -eq 'AMD64') { $Target = 'x86_64-pc-windows-msvc' }
if (-not $Target) { Invoke-DagrFallback "no prebuilt exists for Windows-$Arch" }

$Archive = "$Name-$Target.zip"
$Checksum = "$Name-$Target.sha256"
$Base = if ($env:DAGR_RELEASE_BASE) {
    $env:DAGR_RELEASE_BASE.TrimEnd('/')
} else {
    "https://github.com/$Repo/releases/download/$Tag"
}
$script:TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ("dagr-install-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $script:TmpDir -Force | Out-Null

$HeadCommit = $null
$Git = Get-Command git -ErrorAction SilentlyContinue
if ($Git) {
    & git -C $Root rev-parse --is-inside-work-tree *> $null
    if ($LASTEXITCODE -eq 0) {
        $HeadCommit = ((& git -C $Root rev-parse HEAD 2>$null | Select-Object -First 1) | Out-String).Trim()
        $Dirty = ((& git -C $Root status --porcelain --untracked-files=normal 2>$null) | Out-String).Trim()
        if ($Dirty) { Invoke-DagrFallback 'the checkout has local changes' }
    }
}
if (-not $HeadCommit) {
    $HeadFile = Join-Path $Root '.git\HEAD'
    if (Test-Path -LiteralPath $HeadFile) {
        $Candidate = (Get-Content -LiteralPath $HeadFile -Raw -ErrorAction SilentlyContinue).Trim()
        if ($Candidate -match '^([0-9a-fA-F]{40}|[0-9a-fA-F]{64})$') { $HeadCommit = $Candidate }
    }
}
if (-not $HeadCommit -or $HeadCommit -notmatch '^([0-9a-fA-F]{40}|[0-9a-fA-F]{64})$') {
    Invoke-DagrFallback 'cannot verify the checkout revision'
}

$CommitPath = Join-Path $script:TmpDir 'COMMIT'
if (-not (Invoke-DagrDownload "$Base/COMMIT" $CommitPath)) {
    Invoke-DagrFallback "no release marker is available for $Tag"
}
$ReleaseCommit = (Get-Content -LiteralPath $CommitPath -Raw).Trim()
if ($ReleaseCommit -notmatch '^([0-9a-fA-F]{40}|[0-9a-fA-F]{64})$') {
    Invoke-DagrFallback "the $Tag release marker is malformed"
}
if (-not $HeadCommit.Equals($ReleaseCommit, [System.StringComparison]::OrdinalIgnoreCase)) {
    Invoke-DagrFallback "checkout $HeadCommit does not match the $Tag release revision $ReleaseCommit"
}

$ArchivePath = Join-Path $script:TmpDir $Archive
$ChecksumPath = Join-Path $script:TmpDir $Checksum
[Console]::Out.WriteLine("${Name}: downloading $Archive ($Tag)")
if (-not (Invoke-DagrDownload "$Base/$Archive" $ArchivePath) -or
    -not (Invoke-DagrDownload "$Base/$Checksum" $ChecksumPath)) {
    Invoke-DagrFallback "no prebuilt asset is available for $Tag"
}

$ChecksumLine = (Get-Content -LiteralPath $ChecksumPath | Select-Object -First 1)
$Expected = if ($ChecksumLine) { ($ChecksumLine.Trim() -split '\s+')[0].ToLowerInvariant() } else { $null }
if (-not $Expected -or $Expected -notmatch '^[0-9a-f]{64}$') {
    [Console]::Error.WriteLine("${Name}: malformed checksum asset $Checksum")
    Clear-DagrTemp
    exit 1
}
$Actual = (Get-FileHash -LiteralPath $ArchivePath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($Actual -ne $Expected) {
    [Console]::Error.WriteLine("${Name}: checksum mismatch (expected $Expected, got $Actual)")
    Clear-DagrTemp
    exit 1
}

$Unpack = Join-Path $script:TmpDir 'unpack'
New-Item -ItemType Directory -Path $Unpack -Force | Out-Null
Expand-Archive -LiteralPath $ArchivePath -DestinationPath $Unpack -Force
$SourceBin = Join-Path $Unpack 'dagr.exe'
if (-not (Test-Path -LiteralPath $SourceBin -PathType Leaf)) {
    [Console]::Error.WriteLine("${Name}: release archive does not contain dagr.exe")
    Clear-DagrTemp
    exit 1
}
New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
$InstalledBin = Join-Path $BinDir 'dagr.exe'
Copy-Item -LiteralPath $SourceBin -Destination $InstalledBin -Force
Clear-DagrTemp
[Console]::Out.WriteLine("${Name}: installed $InstalledBin")
exit 0
