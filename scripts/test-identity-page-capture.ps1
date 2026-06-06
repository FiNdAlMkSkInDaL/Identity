$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $repoRoot "target\release\identityd.exe"
$testRoot = Join-Path $repoRoot "tmp\identityd-page-capture-self-test"
$process = $null
$originalClipboard = $null
$hadClipboard = $false
$originalWindowTitle = $null
$hadWindowTitle = $false

if (-not (Test-Path -LiteralPath $exe)) {
    throw "Missing release binary: $exe. Run `cargo build --release -p identityd` first."
}

try {
    $originalClipboard = Get-Clipboard -Raw -ErrorAction Stop
    $hadClipboard = $true
} catch {
    $hadClipboard = $false
}

try {
    $originalWindowTitle = $Host.UI.RawUI.WindowTitle
    $hadWindowTitle = $true
} catch {
    $hadWindowTitle = $false
}

function Get-FreeLoopbackPort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Parse("127.0.0.1"), 0)
    try {
        $listener.Start()
        return ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
    } finally {
        $listener.Stop()
    }
}

try {
    New-Item -ItemType Directory -Force -Path $testRoot | Out-Null

    $port = Get-FreeLoopbackPort
    $addr = "127.0.0.1:$port"

    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $exe
    $psi.WorkingDirectory = $repoRoot
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.Arguments = "--root `"$testRoot`" serve --addr $addr"

    $process = [System.Diagnostics.Process]::Start($psi)

    $healthy = $false
    for ($attempt = 0; $attempt -lt 20; $attempt++) {
        Start-Sleep -Milliseconds 250
        $process.Refresh()
        if ($process.HasExited) {
            $stdout = $process.StandardOutput.ReadToEnd()
            $stderr = $process.StandardError.ReadToEnd()
            throw "identityd serve exited during startup with code $($process.ExitCode)`nstdout=$stdout`nstderr=$stderr"
        }

        try {
            $health = Invoke-RestMethod -Uri "http://$addr/health" -TimeoutSec 2
            if ($health.status -eq "ok") {
                $healthy = $true
                break
            }
        } catch {
            # Keep polling until the loopback endpoint has bound.
        }
    }

    if (-not $healthy) {
        throw "identityd did not answer /health on $addr within 5 seconds"
    }

    Set-Clipboard -Value "plain clipboard text that must not be page-captured implicitly"
    $plainClipboardArgs = @(
        "--root", $testRoot,
        "capture-page",
        "--from-clipboard",
        "--addr", $addr
    )
    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $plainClipboardOutput = (& $exe @plainClipboardArgs 2>&1 | Out-String).Trim()
        $plainClipboardExitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    if ($plainClipboardExitCode -eq 0) {
        throw "capture-page --from-clipboard accepted plain clipboard text without an IDENTITY-PAGE-CAPTURE envelope`n$plainClipboardOutput"
    }
    if ($plainClipboardOutput -notlike "*IDENTITY-PAGE-CAPTURE envelope*") {
        throw "capture-page --from-clipboard did not explain the missing envelope boundary`n$plainClipboardOutput"
    }

    $envelope = @"
[IDENTITY-PAGE-CAPTURE]
Page title: Identity page capture smoke test
Page URL: https://example.test/identity-page-smoke
Selected page text:
Identity page capture smoke test keeps selected browser context local and immediate.
[IDENTITY-PAGE-CAPTURE-END]
"@
    Set-Clipboard -Value $envelope

    $captureArgs = @(
        "--root", $testRoot,
        "capture-page",
        "--from-clipboard",
        "--promote-now",
        "--addr", $addr
    )
    $captureOutput = (& $exe @captureArgs 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "capture-page failed with exit code $LASTEXITCODE`n$captureOutput"
    }
    if ($captureOutput -notmatch "captured_id=\d+") {
        throw "capture-page did not report a captured id`n$captureOutput"
    }
    if ($captureOutput -notmatch "immediate page promotion: .*promoted=1") {
        throw "capture-page --promote-now did not promote exactly one page capture`n$captureOutput"
    }

    $searchArgs = @(
        "--root", $testRoot,
        "memory-search",
        "--query", "Identity page capture smoke test local immediate",
        "--limit", "3"
    )
    $searchOutput = (& $exe @searchArgs 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "memory-search failed with exit code $LASTEXITCODE`n$searchOutput"
    }
    if ($searchOutput -notlike "*Identity page capture smoke test*") {
        throw "promoted page capture was not found in local memory search`n$searchOutput"
    }

    if ($hadWindowTitle) {
        $Host.UI.RawUI.WindowTitle = "Google Gemini - Identity page capture smoke test"
    }

    $contextArgs = @(
        "--root", $testRoot,
        "context-now",
        "--preview",
        "--project", "tfl-central",
        "--limit", "3"
    )
    $contextOutput = (& $exe @contextArgs 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "context-now failed with exit code $LASTEXITCODE`n$contextOutput"
    }
    if ($contextOutput -notlike "*IDENTITY-CONTEXT-BLOCK*") {
        throw "context-now did not produce an Identity context block`n$contextOutput"
    }
    if ($contextOutput -notlike "*Identity page capture smoke test*") {
        throw "promoted page capture was not included in explicit project context`n$contextOutput"
    }

    "identityd page capture self-test passed (pid=$($process.Id), addr=$addr)"
} finally {
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
    }

    if ($hadClipboard) {
        Set-Clipboard -Value $originalClipboard
    } else {
        Set-Clipboard -Value ""
    }

    if ($hadWindowTitle) {
        $Host.UI.RawUI.WindowTitle = $originalWindowTitle
    }

    if (Test-Path -LiteralPath $testRoot) {
        $resolvedRepo = (Resolve-Path -LiteralPath $repoRoot).Path
        $resolvedTestRoot = (Resolve-Path -LiteralPath $testRoot).Path
        if ($resolvedTestRoot.StartsWith($resolvedRepo, [System.StringComparison]::OrdinalIgnoreCase)) {
            Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force
        }
    }
}
