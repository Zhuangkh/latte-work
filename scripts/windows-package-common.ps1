# Shared, side-effect-free checks for Windows desktop packaging.
Set-StrictMode -Version Latest

function Get-PackageHash([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Missing artifact: $Path" }
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Assert-PackageHash([string]$Path, [string]$Expected) {
    if ($Expected -notmatch '^[0-9a-f]{64}$' -or (Get-PackageHash $Path) -cne $Expected) {
        throw "SHA256 mismatch: $Path"
    }
}

function Get-NsisDesktopPatch([string]$Path) {
    # Tauri CLI 2.11.5 patches the first bundle marker, then restores the compiler
    # output. Verify the entire expected payload, not just the changed marker.
    # https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.11.5/crates/tauri-bundler/src/bundle.rs
    $original = '__TAURI_BUNDLE_TYPE_VAR_UNK'
    $replacement = '__TAURI_BUNDLE_TYPE_VAR_NSS'
    $bytes = [IO.File]::ReadAllBytes($Path)
    $text = [Text.Encoding]::Latin1.GetString($bytes)
    $offset = $text.IndexOf($original, [StringComparison]::Ordinal)
    if ($offset -lt 0 -or $text.IndexOf($original, $offset + $original.Length, [StringComparison]::Ordinal) -ge 0) {
        throw 'Expected exactly one unpatched Tauri bundle-type marker'
    }
    [Text.Encoding]::ASCII.GetBytes($replacement).CopyTo($bytes, $offset)
    return [ordered]@{
        source_marker = $original; payload_marker = $replacement; offset = $offset; length = $original.Length
        payload_sha256 = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
        implementation = 'tauri-cli-v2.11.5/crates/tauri-bundler/src/bundle.rs'
    }
}

function Get-PackageSourceIdentity([string]$Root) {
    $head = & git -C $Root rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read source revision' }
    $status = @(& git -C $Root -c core.quotepath=false status --porcelain=v1 --untracked-files=all)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read source status' }
    # Git supplies only tracked/non-ignored source files, excluding build outputs and dependencies.
    $paths = @(& git -C $Root -c core.quotepath=false ls-files --cached --others --exclude-standard)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot enumerate source inputs' }
    $files = @($paths | Sort-Object -Unique | ForEach-Object {
        $path = $_
        $absolute = Join-Path $Root $path
        $hash = if (Test-Path -LiteralPath $absolute -PathType Leaf) { Get-PackageHash $absolute } else { 'deleted' }
        [ordered]@{ path = $path; sha256 = $hash }
    })
    # Documentation/CI edits may continue during a build; compiled inputs must stay stable.
    $buildInputs = @($files | Where-Object {
        $_.path -match '^(apps/|crates/|\.cargo/|Cargo\.|rust-toolchain|package|Makefile$)' -or
        $_.path -match '^scripts/(desktop|package-windows-desktop|verify-windows-package|windows-package-common)\.ps1$'
    })
    $bytes = [System.Text.Encoding]::UTF8.GetBytes(($buildInputs | ConvertTo-Json -Depth 5 -Compress))
    $buildHash = [Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
    return [ordered]@{ head = "$head"; dirty = ($status.Count -gt 0); status = $status; files = $files; build_inputs_sha256 = $buildHash }
}

function Get-ProductionArtifact([string]$LogPath, [string]$Name, [string]$ExpectedPath) {
    $artifacts = @(Get-Content -LiteralPath $LogPath | ForEach-Object {
        if ($_.StartsWith('{')) {
            $message = $_ | ConvertFrom-Json
            if ($message.reason -eq 'compiler-artifact' -and $message.target.name -eq $Name -and $message.executable) {
                $message
            }
        }
    })
    if ($artifacts.Count -ne 1) { throw "Expected one Cargo artifact for $Name, found $($artifacts.Count)" }
    $artifact = $artifacts[0]
    if ($artifact.features -contains 'test-fixture' -or $artifact.profile.test -or $artifact.profile.debug_assertions -or
        $artifact.profile.opt_level -eq '0' -or [IO.Path]::GetFullPath($artifact.executable) -ine [IO.Path]::GetFullPath($ExpectedPath)) {
        throw "Not the expected production release artifact: $Name"
    }
    Get-PackageHash $ExpectedPath | Out-Null
    return $artifact
}

function Get-PackageTargetDirectory([string]$Root) {
    Push-Location $Root
    try {
        $metadata = & cargo metadata --format-version 1 --no-deps --locked
        if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed' }
        return ($metadata | ConvertFrom-Json).target_directory
    } finally { Pop-Location }
}
