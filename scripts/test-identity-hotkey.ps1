$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $repoRoot "target\release\identityd.exe"
$testRoot = Join-Path $repoRoot "tmp\identityd-hotkey-self-test"
$process = $null
$originalClipboard = $null
$hadClipboard = $false

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
    New-Item -ItemType Directory -Force -Path $testRoot | Out-Null
    Set-Clipboard -Value "identityd-hotkey-test-sentinel"

    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $exe
    $psi.WorkingDirectory = $repoRoot
    $psi.Arguments = "--root `"$testRoot`" start"
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true

    $process = [System.Diagnostics.Process]::Start($psi)

    $healthy = $false
    for ($attempt = 0; $attempt -lt 20; $attempt++) {
        Start-Sleep -Milliseconds 500
        $process.Refresh()
        if ($process.HasExited) {
            throw "identityd exited during startup with code $($process.ExitCode)"
        }

        try {
            $health = Invoke-RestMethod -Uri "http://127.0.0.1:8080/health" -TimeoutSec 2
            if ($health.status -eq "ok") {
                $healthy = $true
                break
            }
        } catch {
            # Keep polling until the daemon has bound the endpoint.
        }
    }

    if (-not $healthy) {
        throw "identityd did not answer /health within 10 seconds"
    }

    if (-not ("IdentityHotkeyTestKeys" -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class IdentityHotkeyTestKeys {
  [DllImport("user32.dll")]
  public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo);
}
'@
    }

    [IdentityHotkeyTestKeys]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero)      # Ctrl down
    [IdentityHotkeyTestKeys]::keybd_event(0x10, 0, 0, [UIntPtr]::Zero)      # Shift down
    [IdentityHotkeyTestKeys]::keybd_event(0x49, 0, 0, [UIntPtr]::Zero)      # I down
    Start-Sleep -Milliseconds 80
    [IdentityHotkeyTestKeys]::keybd_event(0x49, 0, 0x0002, [UIntPtr]::Zero) # I up
    [IdentityHotkeyTestKeys]::keybd_event(0x10, 0, 0x0002, [UIntPtr]::Zero) # Shift up
    [IdentityHotkeyTestKeys]::keybd_event(0x11, 0, 0x0002, [UIntPtr]::Zero) # Ctrl up

    Start-Sleep -Seconds 2
    $clipboard = Get-Clipboard -Raw

    if ($clipboard -notlike "*IDENTITY-CONTEXT-BLOCK*") {
        throw "Ctrl+Shift+I did not place an Identity context block on the clipboard"
    }

    "identityd hotkey self-test passed (pid=$($process.Id), clipboard_bytes=$($clipboard.Length))"
} finally {
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
    }

    if ($hadClipboard) {
        Set-Clipboard -Value $originalClipboard
    } else {
        Set-Clipboard -Value ""
    }

    if (Test-Path -LiteralPath $testRoot) {
        $resolvedRepo = (Resolve-Path -LiteralPath $repoRoot).Path
        $resolvedTestRoot = (Resolve-Path -LiteralPath $testRoot).Path
        if ($resolvedTestRoot.StartsWith($resolvedRepo, [System.StringComparison]::OrdinalIgnoreCase)) {
            Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force
        }
    }
}
