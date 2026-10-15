#Requires -Version 7.2
[CmdletBinding()]
param(
    [ValidateSet('All', 'ConPty', 'Ntfs')]
    [string]$Group = 'All',
    [string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
if (-not $IsWindows) { throw 'These experiments require Windows 11.' }
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run as an ordinary, non-elevated user.'
}
$repo = Split-Path $PSScriptRoot -Parent
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $repo 'target/windows-terminal-undo-experiments'
}
$null = New-Item -ItemType Directory -Force -Path $OutputDirectory
$OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path
$os = Get-CimInstance Win32_OperatingSystem
if ([int]$os.BuildNumber -lt 22000) { throw 'Windows 10 validation is deferred; use Windows 11.' }
$pwsh = (Get-Command pwsh.exe -CommandType Application).Source
$tempDrive = [IO.DriveInfo]::new([IO.Path]::GetPathRoot([IO.Path]::GetTempPath()))
if ($tempDrive.DriveFormat -ne 'NTFS') { throw 'The experiment temp directory must be on NTFS.' }
Push-Location $repo
try {
    [ordered]@{
        os = $os.Caption
        build = $os.BuildNumber
        architecture = $os.OSArchitecture
        pwsh = $pwsh
        pwshVersion = (& $pwsh -NoLogo -NoProfile -Command '$PSVersionTable.PSVersion.ToString()')
        elevated = $false
        tempFileSystem = $tempDrive.DriveFormat
        rust = (& rustc --version)
        head = (& git rev-parse HEAD)
        startedAt = [DateTimeOffset]::Now.ToString('o')
    } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $OutputDirectory "environment-$Group.json") -Encoding utf8
    $groups = if ($Group -eq 'All') { @('ConPty', 'Ntfs') } else { @($Group) }
    foreach ($item in $groups) {
        $filter = "win32::experiments::$($item.ToLowerInvariant())"
        $log = Join-Path $OutputDirectory "$($item.ToLowerInvariant()).log"
        Write-Output "Running $item; log: $log"
        & cargo test -p latte-work-windows-security --lib --locked $filter -- --ignored --nocapture --test-threads=1 --skip _worker 2>&1 |
            Tee-Object -FilePath $log
        if ($LASTEXITCODE -ne 0) { throw "$item experiment failed; inspect $log" }
    }
} finally {
    Pop-Location
}
