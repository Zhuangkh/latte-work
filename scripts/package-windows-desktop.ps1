# Build an unsigned x64 per-user NSIS installer. Never runs the installer or Server.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'windows-package-common.ps1')
$runDir = $null
$sourceBefore = $null
$sourceAfter = $null
Push-Location $root
try {
    if (-not $IsWindows) { throw 'Windows desktop packaging requires native Windows' }
    $rustVersion = @(& rustc -vV)
    if ($LASTEXITCODE -ne 0) { throw 'rustc is required' }
    $triple = ($rustVersion | Select-String '^host: ').ToString().Substring(6).Trim()
    if ($triple -ne 'x86_64-pc-windows-msvc') { throw "Unsupported package target: $triple (requires x86_64-pc-windows-msvc)" }
    $cli = Join-Path $root 'node_modules/@tauri-apps/cli/tauri.js'
    $cliPackage = Join-Path $root 'node_modules/@tauri-apps/cli/package.json'
    if (-not (Test-Path -LiteralPath $cli)) { throw 'Missing locked Tauri CLI; run npm ci first' }
    $lock = Get-Content -Raw package-lock.json | ConvertFrom-Json -AsHashtable
    $cliVersion = (Get-Content -Raw $cliPackage | ConvertFrom-Json).version
    if ($cliVersion -ne $lock.packages.'node_modules/@tauri-apps/cli'.version) { throw 'Installed Tauri CLI differs from package-lock.json; run npm ci' }
    if ($cliVersion -ne '2.11.5') { throw 'NSIS payload verification supports locked Tauri CLI 2.11.5; review bundle-type patch behavior before updating it' }
    $config = Get-Content -Raw apps/desktop/src-tauri/tauri.conf.json | ConvertFrom-Json
    $targetDir = Get-PackageTargetDirectory $root
    $runDir = Join-Path $targetDir ('windows-package/' + [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ') + '-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $runDir | Out-Null
    $sourceBefore = Get-PackageSourceIdentity $root
    $sourceBefore | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $runDir 'source-before.json') -Encoding utf8
    $releaseDir = Join-Path $targetDir "$triple/release"
    $server = Join-Path $releaseDir 'latte-work-server.exe'
    $serverLog = Join-Path $runDir 'server-build.jsonl'
    Write-Host "Building production Server ($triple); evidence: $runDir"
    & cargo build --release -p latte-work-server --target $triple --locked --message-format=json-render-diagnostics 2>&1 |
        ForEach-Object { $line = $_.ToString(); if (-not $line.StartsWith('{')) { Write-Host $line }; $line } |
        Set-Content -LiteralPath $serverLog -Encoding utf8
    if ($LASTEXITCODE -ne 0) { throw "Server release build failed; see $serverLog" }
    $serverArtifact = Get-ProductionArtifact $serverLog 'latte-work-server' $server
    $serverHash = Get-PackageHash $server
    $sidecar = Join-Path $root "apps/desktop/src-tauri/binaries/latte-work-server-$triple.exe"
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $sidecar) | Out-Null
    Copy-Item -LiteralPath $server -Destination $sidecar -Force
    Assert-PackageHash $sidecar $serverHash
    $installerName = "$($config.productName)_$($config.version)_x64-setup.exe"
    $bundledInstaller = Join-Path $releaseDir "bundle/nsis/$installerName"
    # Preserve the exact prior output, so a successful command cannot accidentally publish a stale installer.
    if (Test-Path -LiteralPath $bundledInstaller) {
        $preserved = Join-Path $targetDir ('windows-package-preserved/' + (Split-Path -Leaf $runDir))
        New-Item -ItemType Directory -Path $preserved -Force | Out-Null
        Move-Item -LiteralPath $bundledInstaller -Destination (Join-Path $preserved $installerName)
    }
    $desktopLog = Join-Path $runDir 'desktop-build.jsonl'
    Write-Host 'Building release desktop and unsigned NSIS installer (thin LTO can take several minutes)'
    Push-Location (Join-Path $root 'apps/desktop')
    try {
        & node $cli build --ci --no-sign --target $triple --bundles nsis -- --locked --message-format=json-render-diagnostics 2>&1 |
            ForEach-Object { $line = $_.ToString(); if (-not $line.StartsWith('{')) { Write-Host $line }; $line } |
            Set-Content -LiteralPath $desktopLog -Encoding utf8
        if ($LASTEXITCODE -ne 0) { throw "Tauri package build failed; see $desktopLog" }
    } finally { Pop-Location }
    $desktop = Join-Path $releaseDir 'latte-work-desktop.exe'
    $desktopArtifact = Get-ProductionArtifact $desktopLog 'latte-work-desktop' $desktop
    $sourceAfter = Get-PackageSourceIdentity $root
    Write-PackageSourceEvidence $sourceBefore $sourceAfter $runDir
    Assert-PackageSourceUnchanged $sourceBefore $sourceAfter
    Assert-PackageHash $server $serverHash
    Assert-PackageHash $sidecar $serverHash
    $installer = Join-Path $runDir $installerName
    Copy-Item -LiteralPath $bundledInstaller -Destination $installer
    $installerHash = Get-PackageHash $installer
    "$installerHash  $installerName" | Set-Content -LiteralPath "$installer.sha256" -Encoding ascii
    $tools = [ordered]@{ rustc = ($rustVersion -join "`n"); cargo = (& cargo --version); node = (& node --version); npm = (& npm --version); tauri_cli = $cliVersion; powershell = $PSVersionTable.PSVersion.ToString() }
    $provenance = [ordered]@{
        schema_version = 1; created_utc = [DateTime]::UtcNow.ToString('o'); version = $config.version
        target = $triple; profile = 'release'; installer_type = 'nsis'; install_mode = 'currentUser'; signed = $false
        webview_install_mode = 'downloadBootstrapper'; source = $sourceBefore; source_after = $sourceAfter; tools = $tools
        server = @{ path = $server; sha256 = $serverHash; features = @($serverArtifact.features); cargo_log = $serverLog }
        desktop = @{ path = $desktop; sha256 = (Get-PackageHash $desktop); nsis_patch = (Get-NsisDesktopPatch $desktop); features = @($desktopArtifact.features); cargo_log = $desktopLog }
        installer = @{ path = $installer; sha256 = $installerHash; checksum_path = "$installer.sha256" }
    }
    $provenancePath = Join-Path $runDir 'provenance.json'
    $provenance | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $provenancePath -Encoding utf8
    Write-Host "Package provenance: $provenancePath"
    Write-Host "Installer: $installer`nSHA256: $installerHash"
} finally {
    try {
        # Build failures still need an after snapshot; never create a successful receipt here.
        if ($null -ne $sourceBefore -and $null -eq $sourceAfter) {
            $sourceAfter = Get-PackageSourceIdentity $root
            Write-PackageSourceEvidence $sourceBefore $sourceAfter $runDir
        }
    } finally { Pop-Location }
}
