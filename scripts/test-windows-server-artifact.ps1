# Dependency-free contract tests; optionally verify/tamper with copies of a real release.
[CmdletBinding()]
param([string]$ArtifactDirectory)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$testArtifactDirectory = $ArtifactDirectory
. (Join-Path $PSScriptRoot 'build-windows-server.ps1')

$checks = 0
function Assert-Rejected([string]$Name, [scriptblock]$Action, [string]$Pattern) {
    try { & $Action | Out-Null } catch {
        if ($_.Exception.Message -notmatch $Pattern) { throw "$Name failed for the wrong reason: $_" }
        Write-Host "PASS: $Name"
        $script:checks++
        return
    }
    throw "$Name unexpectedly succeeded"
}

$temporary = Join-Path ([IO.Path]::GetTempPath()) ("latte-server-artifact-tests-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $temporary | Out-Null
try {
    $binary = Join-Path $temporary 'latte-work-server.exe'
    [IO.File]::WriteAllBytes($binary, [byte[]]@(77, 90, 1))
    $packageId = 'path+file:///server#latte-work-server@0.1.0'
    $message = @{
        reason = 'compiler-artifact'; package_id = $packageId; features = @()
        executable = $binary; target = @{ name = 'latte-work-server'; kind = @('bin') }
        profile = @{ test = $false; debug_assertions = $false; opt_level = '3' }
    }
    $finished = '{"reason":"build-finished","success":true}'
    $json = $message | ConvertTo-Json -Depth 5 -Compress
    Select-ProductionServerArtifact @($json, $finished) $packageId $binary | Out-Null
    Write-Host 'PASS: exact production compiler artifact accepted'
    $checks++
    Assert-Rejected 'missing compiler artifact' { Select-ProductionServerArtifact @($finished) $packageId $binary } 'Missing successful'
    Assert-Rejected 'failed Cargo completion' { Select-ProductionServerArtifact @($json, '{"reason":"build-finished","success":false}') $packageId $binary } 'Missing successful'
    Assert-Rejected 'wrong executable path' { Select-ProductionServerArtifact @($json, $finished) $packageId (Join-Path $temporary 'stale.exe') } 'unexpected.*path'
    $message.features = @('test-fixture')
    Assert-Rejected 'test-fixture feature' { Select-ProductionServerArtifact @(($message | ConvertTo-Json -Depth 5 -Compress), $finished) $packageId $binary } 'no enabled features'
    $message.features = @()
    $message.target.name = 'native-claude-fixture'
    Assert-Rejected 'fixture executable' { Select-ProductionServerArtifact @(($message | ConvertTo-Json -Depth 5 -Compress), $finished) $packageId $binary } 'Unexpected executable'
    $message.target.name = 'latte-work-server'
    $message.profile.test = $true
    Assert-Rejected 'test profile' { Select-ProductionServerArtifact @(($message | ConvertTo-Json -Depth 5 -Compress), $finished) $packageId $binary } 'production release profile'
    $message.profile.test = $false
    Remove-Item -LiteralPath $binary
    Assert-Rejected 'missing reported executable' { Select-ProductionServerArtifact @($json, $finished) $packageId $binary } 'does not exist|Cannot find'
    [IO.File]::WriteAllBytes($binary, [byte[]]@())
    Assert-Rejected 'empty reported executable' { Select-ProductionServerArtifact @($json, $finished) $packageId $binary } 'empty'
    Assert-Rejected 'native nonzero exit' { Invoke-ServerTool (Get-Process -Id $PID).Path @('-NoLogo', '-NoProfile', '-Command', 'exit 7') } 'exit code 7'
    # Existing output must be refused before Cargo can reuse any stale artifact.
    Assert-Rejected 'existing output directory' { Build-ServerRelease $temporary } 'existing artifact directory'

    if ($testArtifactDirectory) {
        Test-ServerRelease $testArtifactDirectory | Out-Null
        Write-Host 'PASS: real production artifact, checksum, provenance and --help'
        $checks++
        $copy = Join-Path $temporary 'copy'
        New-Item -ItemType Directory -Path $copy | Out-Null
        Get-ChildItem -LiteralPath $testArtifactDirectory | Copy-Item -Destination $copy
        $copyBinary = Join-Path $copy 'latte-work-server.exe'
        $checksumPath = "$copyBinary.sha256"
        $checksum = Get-Content -LiteralPath $checksumPath -Raw
        Set-Content -LiteralPath $checksumPath -Value ('0' * 64 + '  latte-work-server.exe')
        Assert-Rejected 'wrong checksum' { Test-ServerRelease $copy } 'mismatch'
        Set-Content -LiteralPath $checksumPath -Value $checksum.Trim()
        $provenancePath = Join-Path $copy 'provenance.json'
        $provenanceText = Get-Content -LiteralPath $provenancePath -Raw
        $provenance = $provenanceText | ConvertFrom-Json
        $provenance.features = @('test-fixture')
        $provenance | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $provenancePath
        Assert-Rejected 'fixture provenance' { Test-ServerRelease $copy } 'Invalid production'
        Set-Content -LiteralPath $provenancePath -Value $provenanceText
        $stream = [IO.File]::OpenWrite($copyBinary)
        try { $stream.WriteByte(0) } finally { $stream.Dispose() }
        Assert-Rejected 'stale or changed executable' { Test-ServerRelease $copy } 'mismatch'
        Remove-Item -LiteralPath $copyBinary
        Assert-Rejected 'missing packaged executable' { Test-ServerRelease $copy } 'exactly the executable'
    }
    Write-Host "Passed $checks Server artifact contract checks."
} finally {
    # Only remove the unique directory created above, never a supplied artifact/cache.
    $resolved = [IO.Path]::GetFullPath($temporary)
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -or
        [IO.Path]::GetFileName($resolved) -notmatch '^latte-server-artifact-tests-[0-9a-f]{32}$') { throw 'Unsafe temporary cleanup path' }
    Remove-Item -LiteralPath $resolved -Recurse
}
