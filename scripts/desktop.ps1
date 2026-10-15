# Windows desktop entry point. The macOS/Linux equivalent is scripts/desktop.sh.
# Tauri validates the external sidecar and icon.ico before compiling the desktop,
# so every mode prepares the server binary under apps/desktop/src-tauri/binaries.
[CmdletBinding()]
param(
    [ValidateSet('prepare', 'check', 'test', 'build', 'dev', 'package')]
    [string]$Mode = 'dev'
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

. (Join-Path $PSScriptRoot 'windows-package-common.ps1')
$targetDir = Get-PackageTargetDirectory $root
$triple = (& rustc -vV | Select-String '^host: ').ToString().Substring(6).Trim()
if (-not $triple) { throw 'Cannot determine the Rust host triple' }

function Invoke-Prepare {
    & cargo build -p latte-work-server --locked
    if ($LASTEXITCODE -ne 0) { throw 'Server build failed' }
    $source = Join-Path $targetDir 'debug\latte-work-server.exe'
    if (-not (Test-Path $source)) { throw "Missing $source" }
    $binaries = Join-Path $root 'apps\desktop\src-tauri\binaries'
    New-Item -ItemType Directory -Force -Path $binaries | Out-Null
    Copy-Item -Force $source (Join-Path $binaries "latte-work-server-$triple.exe")
}

function Invoke-Cargo([string[]]$Arguments, [string]$Failure) {
    & cargo @Arguments
    if ($LASTEXITCODE -ne 0) { throw $Failure }
}

switch ($Mode) {
    'package' { & (Join-Path $PSScriptRoot 'package-windows-desktop.ps1') }
    'prepare' { Invoke-Prepare }
    'check' {
        Invoke-Prepare
        Invoke-Cargo @('check', '-p', 'latte-work-desktop', '--all-targets', '--locked') 'Desktop check failed'
    }
    'test' {
        Invoke-Prepare
        Invoke-Cargo @('test', '-p', 'latte-work-desktop', '--locked') 'Desktop tests failed'
    }
    'build' {
        Invoke-Prepare
        Invoke-Cargo @('build', '-p', 'latte-work-desktop', '--locked') 'Desktop build failed'
    }
    'dev' {
        Invoke-Prepare
        Push-Location (Join-Path $root 'apps\desktop')
        try {
            & npm exec -- tauri dev
            if ($LASTEXITCODE -ne 0) { throw 'tauri dev failed' }
        } finally {
            Pop-Location
        }
    }
}
