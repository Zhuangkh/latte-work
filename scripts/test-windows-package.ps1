# Focused package validation tests; synthetic files never execute.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'windows-package-common.ps1')
$testDir = Join-Path (Get-PackageTargetDirectory (Split-Path -Parent $PSScriptRoot)) ('windows-package-tests/' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testDir | Out-Null
$binary = Join-Path $testDir 'latte-work-server.exe'
Set-Content -LiteralPath $binary -Value 'synthetic release bytes' -NoNewline
$hash = Get-PackageHash $binary
Assert-PackageHash $binary $hash
$script:passed = 1
function Assert-Rejected([string]$Name, [scriptblock]$Action) {
    $rejected = $false
    try { & $Action | Out-Null } catch { $rejected = $true }
    if (-not $rejected) { throw "Expected rejection: $Name" }
    $script:passed++
    Write-Host "PASS: rejects $Name"
}
Assert-Rejected 'missing file' { Get-PackageHash (Join-Path $testDir 'absent.exe') }
Assert-Rejected 'stale/mismatched bytes' { Assert-PackageHash $binary ('0' * 64) }
$log = Join-Path $testDir 'cargo.jsonl'
function Write-Artifact([string[]]$Features = @(), [bool]$Debug = $false, [bool]$Test = $false, [string]$Path = $binary) {
    @{ reason = 'compiler-artifact'; target = @{ name = 'latte-work-server' }; executable = $Path
        features = $Features; profile = @{ test = $Test; debug_assertions = $Debug; opt_level = '3' } } |
        ConvertTo-Json -Compress -Depth 4 | Set-Content -LiteralPath $log
}
Write-Artifact
Get-ProductionArtifact $log 'latte-work-server' $binary | Out-Null
$script:passed++
Write-Artifact -Features @('test-fixture')
Assert-Rejected 'fixture-enabled release' { Get-ProductionArtifact $log 'latte-work-server' $binary }
Write-Artifact -Debug $true
Assert-Rejected 'debug artifact' { Get-ProductionArtifact $log 'latte-work-server' $binary }
Write-Artifact -Test $true
Assert-Rejected 'test artifact' { Get-ProductionArtifact $log 'latte-work-server' $binary }
Write-Artifact -Path (Join-Path $testDir 'other.exe')
Assert-Rejected 'unexpected artifact path' { Get-ProductionArtifact $log 'latte-work-server' $binary }
Set-Content -LiteralPath $log -Value ''
Assert-Rejected 'missing compiler artifact' { Get-ProductionArtifact $log 'latte-work-server' $binary }
Write-Artifact
$line = Get-Content -LiteralPath $log
Add-Content -LiteralPath $log -Value $line
Assert-Rejected 'ambiguous compiler artifacts' { Get-ProductionArtifact $log 'latte-work-server' $binary }
$desktop = Join-Path $testDir 'desktop.exe'
$payload = Join-Path $testDir 'payload.exe'
Set-Content -LiteralPath $desktop -Value 'prefix__TAURI_BUNDLE_TYPE_VAR_UNKsuffix' -NoNewline
$patch = Get-NsisDesktopPatch $desktop
if ($patch.offset -ne 6 -or $patch.length -ne 27) { throw 'Wrong NSIS patch boundary' }
Set-Content -LiteralPath $payload -Value 'prefix__TAURI_BUNDLE_TYPE_VAR_NSSsuffix' -NoNewline
Assert-PackageHash $payload $patch.payload_sha256
$script:passed++
Set-Content -LiteralPath $payload -Value 'PrefiX__TAURI_BUNDLE_TYPE_VAR_NSSsuffix' -NoNewline
Assert-Rejected 'NSIS marker plus unrelated byte tampering' { Assert-PackageHash $payload $patch.payload_sha256 }
Set-Content -LiteralPath $payload -Value 'prefix__TAURI_BUNDLE_TYPE_VAR_NSSsuffixEXTRA' -NoNewline
Assert-Rejected 'NSIS marker plus appended bytes' { Assert-PackageHash $payload $patch.payload_sha256 }
Set-Content -LiteralPath $desktop -Value 'prefix__TAURI_BUNDLE_TYPE_VAR_NSSsuffix' -NoNewline
Assert-Rejected 'missing original Tauri marker' { Get-NsisDesktopPatch $desktop }
Set-Content -LiteralPath $desktop -Value '__TAURI_BUNDLE_TYPE_VAR_UNK__TAURI_BUNDLE_TYPE_VAR_UNK' -NoNewline
Assert-Rejected 'ambiguous original Tauri markers' { Get-NsisDesktopPatch $desktop }

# Borrow only Git objects/HEAD for identity tests; no commit, build or dependency copy.
$root = Split-Path -Parent $PSScriptRoot
$sourceFixture = Join-Path $testDir 'source'
& git clone --quiet --shared --no-checkout $root $sourceFixture
if ($LASTEXITCODE -ne 0) { throw 'Cannot create isolated source identity fixture' }
$inputPaths = @('Cargo.toml', 'apps/desktop/src-tauri/Cargo.toml', 'crates/latte-work-platform/src/lib.rs', 'apps/desktop/src-tauri/tauri.conf.json')
& git -C $sourceFixture checkout HEAD -- @inputPaths
if ($LASTEXITCODE -ne 0) { throw 'Cannot populate source identity fixture' }
Copy-Item -LiteralPath (Join-Path $root '.gitattributes') -Destination $sourceFixture
$before = Get-PackageSourceIdentity $sourceFixture
Assert-PackageSourceUnchanged $before (Get-PackageSourceIdentity $sourceFixture)
$script:passed++
foreach ($path in @($inputPaths + '.gitattributes')) {
    $absolute = Join-Path $sourceFixture $path
    $original = [IO.File]::ReadAllBytes($absolute)
    try {
        $text = [IO.File]::ReadAllText($absolute)
        $changed = switch ($path) {
            'Cargo.toml' { $text.Replace('version = "0.1.0"', 'version = "0.1.1"') }
            'apps/desktop/src-tauri/Cargo.toml' { $text.Replace('features = ["unstable"]', 'features = ["unstable", "tray-icon"]') }
            'apps/desktop/src-tauri/tauri.conf.json' {
                $config = $text | ConvertFrom-Json
                $config.version = '0.1.1'
                $config | ConvertTo-Json -Depth 12
            }
            'crates/latte-work-platform/src/lib.rs' { $text + "`npub const PACKAGE_INPUT_TEST: bool = true;`n" }
            '.gitattributes' { $text + "`n*.rs text eol=lf`n" }
        }
        if ($changed -ceq $text) { throw "Source fixture mutation was ineffective: $path" }
        [IO.File]::WriteAllText($absolute, $changed)
        $after = Get-PackageSourceIdentity $sourceFixture
        Assert-Rejected "changed $path" { Assert-PackageSourceUnchanged $before $after }
        Write-PackageSourceEvidence $before $after $testDir
        $report = Get-Content -LiteralPath (Join-Path $testDir 'source-changes.txt') -Raw
        if (-not $report.Contains($path) -or -not $report.Contains((Get-PackageHash $absolute))) {
            throw "Source evidence omitted changed input: $path"
        }
    } finally { [IO.File]::WriteAllBytes($absolute, $original) }
}
$added = Join-Path $sourceFixture 'apps/desktop/new-package-input.rs'
Set-Content -LiteralPath $added -Value 'new Rust source'
Assert-Rejected 'added source input' { Assert-PackageSourceUnchanged $before (Get-PackageSourceIdentity $sourceFixture) }
$restored = Get-PackageSourceIdentity $sourceFixture
$beforeDifferentHead = [ordered]@{ head = 'different revision'; build_inputs_sha256 = $restored.build_inputs_sha256 }
Assert-Rejected 'changed HEAD' { Assert-PackageSourceUnchanged $beforeDifferentHead $restored }
Write-Host "PASS: $script:passed focused package checks. Evidence retained at $testDir"
