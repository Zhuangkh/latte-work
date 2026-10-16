# Verify the same-user policy under an already elevated Windows token.
# This never requests elevation, changes accounts or touches a user's running Server.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'Elevated-token acceptance requires native Windows' }
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this acceptance gate in an already elevated PowerShell; ordinary-user tests are separate'
}
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    Write-Host "Elevated same-user acceptance: $($identity.User.Value)"
    & cargo test -p latte-work-windows-security -p latte-work-platform --lib --locked -- --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw 'Elevated security/platform tests failed' }
    & cargo test -p latte-work-server --features test-fixture --test native ssh:: --locked -- --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw 'Elevated Server/SSH launcher tests failed' }
} finally {
    Pop-Location
}
