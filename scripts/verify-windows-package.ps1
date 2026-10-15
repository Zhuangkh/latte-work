# Inspect NSIS bytes without installing, launching the desktop, or starting a daemon.
[CmdletBinding()]
param([string]$ProvenancePath)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'windows-package-common.ps1')
if (-not $ProvenancePath) {
    $packages = Join-Path (Get-PackageTargetDirectory $root) 'windows-package'
    $latest = Get-ChildItem -LiteralPath $packages -Filter provenance.json -Recurse -File |
        Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 1
    if (-not $latest) { throw "No successful package provenance under $packages" }
    $ProvenancePath = $latest.FullName
}
$p = Get-Content -LiteralPath $ProvenancePath -Raw | ConvertFrom-Json
if ($p.schema_version -ne 1 -or $p.target -ne 'x86_64-pc-windows-msvc' -or $p.profile -ne 'release' -or
    $p.installer_type -ne 'nsis' -or $p.install_mode -ne 'currentUser' -or $p.signed -ne $false -or
    $p.webview_install_mode -ne 'downloadBootstrapper' -or $p.tools.tauri_cli -ne '2.11.5' -or $p.server.features -contains 'test-fixture' -or
    $p.source.head -ne $p.source_after.head -or $p.source.build_inputs_sha256 -ne $p.source_after.build_inputs_sha256) {
    throw 'Provenance does not describe a stable unsigned x64 production NSIS package'
}
foreach ($artifact in @($p.installer, $p.desktop, $p.server)) { Assert-PackageHash $artifact.path $artifact.sha256 }
$checksum = (Get-Content -LiteralPath $p.installer.checksum_path -Raw).Trim()
if ($checksum -cne "$($p.installer.sha256)  $([IO.Path]::GetFileName($p.installer.path))") { throw 'Installer checksum receipt mismatch' }
Get-ProductionArtifact $p.server.cargo_log 'latte-work-server' $p.server.path | Out-Null
Get-ProductionArtifact $p.desktop.cargo_log 'latte-work-desktop' $p.desktop.path | Out-Null
$sevenZip = Get-Command 7z.exe -ErrorAction SilentlyContinue
$extractor = if ($sevenZip) { $sevenZip.Source } else { Join-Path $env:ProgramFiles '7-Zip/7z.exe' }
if (-not (Test-Path -LiteralPath $extractor -PathType Leaf)) { throw '7-Zip (7z.exe) is required for package inspection' }
$inspection = Join-Path (Split-Path -Parent $ProvenancePath) ('inspection-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $inspection | Out-Null
& $extractor x $p.installer.path "-o$inspection" -y | Set-Content -LiteralPath (Join-Path $inspection '7zip.log')
if ($LASTEXITCODE -ne 0) { throw "Installer extraction failed; see $inspection/7zip.log" }
$desktops = @(Get-ChildItem -LiteralPath $inspection -Filter latte-work-desktop.exe -Recurse -File)
$servers = @(Get-ChildItem -LiteralPath $inspection -Filter latte-work-server.exe -Recurse -File)
if ($desktops.Count -ne 1 -or $servers.Count -ne 1) { throw 'Installer must contain exactly one desktop and one untripled production Server' }
if ($desktops[0].DirectoryName -ine $servers[0].DirectoryName) { throw 'Installed Server is not adjacent to desktop (release resolver layout)' }
$patch = Get-NsisDesktopPatch $p.desktop.path
foreach ($field in @('source_marker', 'payload_marker', 'offset', 'length', 'payload_sha256', 'implementation')) {
    if ($p.desktop.nsis_patch.$field -cne $patch.$field) { throw "NSIS patch provenance mismatch: $field" }
}
Assert-PackageHash $desktops[0].FullName $patch.payload_sha256
Assert-PackageHash $servers[0].FullName $p.server.sha256
$version = $desktops[0].VersionInfo.ProductVersion
if ($version -ne $p.version) { throw "Embedded desktop version $version differs from provenance $($p.version)" }
$unexpected = @(Get-ChildItem -LiteralPath $inspection -Recurse -File | Where-Object { $_.Name -match '(test-fixture|latte-work-server-.+\.exe$)' })
if ($unexpected.Count -gt 0) { throw 'Installer contains fixture or incorrectly named sidecar' }
$receipt = [ordered]@{
    verified_utc = [DateTime]::UtcNow.ToString('o'); provenance = [IO.Path]::GetFullPath($ProvenancePath)
    installer_sha256 = $p.installer.sha256; desktop_compiler_sha256 = $p.desktop.sha256
    desktop_payload_sha256 = $patch.payload_sha256; desktop_nsis_patch = $patch; server_sha256 = $p.server.sha256
    extracted_desktop = $desktops[0].FullName; extracted_server = $servers[0].FullName; version = $version
    scope = 'unsigned x64 package built and extracted; installation and native UI not exercised'
}
$receipt | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $inspection 'verification.json') -Encoding utf8
Write-Host "PASS: unsigned x64 NSIS package, embedded desktop/Server hashes and release resolver layout verified. Receipt: $inspection/verification.json"
