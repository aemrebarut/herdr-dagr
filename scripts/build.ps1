# Local Windows development build: produce the Cargo artifact and the stable
# plugin launch path used by herdr-plugin.toml.
$ErrorActionPreference = 'Stop'

$Root = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if ($Root.StartsWith('\\?\')) { $Root = $Root.Substring(4) }
$Cargo = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $Cargo) {
    [Console]::Error.WriteLine('dagr: Cargo is required to build this source')
    exit 1
}

Push-Location $Root
try {
    & cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $BinDir = Join-Path $Root 'bin'
    New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $Root 'target\release\dagr.exe') `
        -Destination (Join-Path $BinDir 'dagr.exe') -Force
} finally {
    Pop-Location
}
