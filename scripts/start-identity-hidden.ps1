param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $IdentityArgs
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $repoRoot "target\release\identityd.exe"

if (-not (Test-Path -LiteralPath $exe)) {
    throw "Missing release binary: $exe. Run `cargo build --release -p identityd` first."
}

function Quote-IdentityArg {
    param([string] $Value)

    if ($Value -match '^[A-Za-z0-9_./:=+,-]+$') {
        return $Value
    }

    return '"' + ($Value -replace '"', '\"') + '"'
}

$argsForIdentity = @("start") + $IdentityArgs
$psi = [System.Diagnostics.ProcessStartInfo]::new()
$psi.FileName = $exe
$psi.WorkingDirectory = $repoRoot
$psi.Arguments = ($argsForIdentity | ForEach-Object { Quote-IdentityArg $_ }) -join " "
$psi.UseShellExecute = $true
$psi.WindowStyle = [System.Diagnostics.ProcessWindowStyle]::Hidden

$process = [System.Diagnostics.Process]::Start($psi)
Start-Sleep -Milliseconds 750
$process.Refresh()

if ($process.HasExited) {
    throw "identityd exited immediately with code $($process.ExitCode). Try .\target\release\identityd.exe start for visible logs."
}

"identityd started hidden (pid=$($process.Id), hotkey=Ctrl+Shift+I, health=http://127.0.0.1:8080/health)"
