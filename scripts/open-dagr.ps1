# Windows launcher for dagr's Herdr actions. Herdr preview cannot spawn a
# plugin-relative pane executable on Windows, so this script opens a shell
# pane and runs dagr.exe by absolute plugin-root path. It preserves the
# user's run-file cwd and implements right/down/left/up placement.
param(
    [ValidateSet('right', 'down', 'left', 'up')]
    [string]$Placement = 'right'
)

$ErrorActionPreference = 'Stop'
$Utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = $Utf8NoBom
$OutputEncoding = $Utf8NoBom

$HerdrBin = if ($env:HERDR_BIN_PATH) { $env:HERDR_BIN_PATH } else { 'herdr' }
$PluginRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if ($PluginRoot.StartsWith('\\?\')) { $PluginRoot = $PluginRoot.Substring(4) }
$DagrBin = Join-Path $PluginRoot 'bin\dagr.exe'

$Direction = 'right'
$Swap = $null
switch ($Placement) {
    'down' { $Direction = 'down' }
    'left' { $Direction = 'right'; $Swap = 'left' }
    'up' { $Direction = 'down'; $Swap = 'up' }
}

function Strip-Verbatim([string]$Path) {
    if ($Path -and $Path.StartsWith('\\?\')) { return $Path.Substring(4) }
    return $Path
}

# Query before splitting: the focused pane is still the user's work pane.
$RunCwd = $null
try {
    $Focused = (& $HerdrBin pane list | ConvertFrom-Json).result.panes |
        Where-Object { $_.focused } | Select-Object -First 1
    if ($Focused -and $Focused.cwd) { $RunCwd = Strip-Verbatim $Focused.cwd }
} catch {}
if (-not $RunCwd -and $env:HERDR_ACTIVE_PANE_CWD) {
    $RunCwd = Strip-Verbatim $env:HERDR_ACTIVE_PANE_CWD
}
if (-not $RunCwd) { $RunCwd = $PluginRoot }

if (-not (Test-Path -LiteralPath $DagrBin -PathType Leaf)) {
    [Console]::Error.WriteLine("dagr: installed binary not found at $DagrBin")
    exit 1
}

$Out = (& $HerdrBin pane split --direction $Direction --cwd $RunCwd --focus | Out-String)
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$PaneId = ([regex]'"pane_id"\s*:\s*"([^"]+)"').Match($Out).Groups[1].Value
if (-not $PaneId) {
    [Console]::Error.WriteLine('dagr: Herdr did not return the new pane id')
    exit 1
}

# `pane run` types into the pane's PowerShell. The call operator and quoted
# absolute path survive spaces in an install directory.
$Command = "& `"$DagrBin`" view"
& $HerdrBin pane run $PaneId $Command
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& $HerdrBin pane rename $PaneId dagr *> $null

if ($Swap) {
    & $HerdrBin pane swap --pane $PaneId --direction $Swap *> $null
    exit $LASTEXITCODE
}
exit 0
