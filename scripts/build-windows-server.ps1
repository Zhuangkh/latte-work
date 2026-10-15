# Build and verify a standalone production Server without touching desktop sidecars.
[CmdletBinding()]
param(
    [ValidateSet('Build', 'Verify')]
    [string]$Mode = 'Build',
    [string]$ArtifactDirectory
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-ServerTool([string]$Tool, [string[]]$Arguments) {
    $output = & $Tool @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Tool failed with exit code $LASTEXITCODE" }
    return $output
}

function Get-ServerSourceIdentity([string]$Root) {
    $head = Invoke-ServerTool git @('-C', $Root, 'rev-parse', 'HEAD')
    $status = @(Invoke-ServerTool git @('-C', $Root, 'status', '--porcelain=v1', '--untracked-files=all'))
    $paths = @(Invoke-ServerTool git @('-C', $Root, '-c', 'core.quotePath=false', 'ls-files', '--cached', '--others', '--exclude-standard'))
    $entries = foreach ($path in ($paths | Sort-Object -Unique)) {
        $file = Join-Path $Root $path
        $hash = if (Test-Path -LiteralPath $file -PathType Leaf) {
            (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
        } else { 'missing' }
        "$hash  $path"
    }
    $bytes = [Text.Encoding]::UTF8.GetBytes(($entries -join "`n"))
    $sourceHash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
    # Only build inputs must stay stable while another worker edits unrelated docs/UI.
    $inputs = @($entries | Where-Object { $_ -match '  (Cargo\.(toml|lock)|rust-toolchain(\.toml)?|\.cargo/.*|crates/.*|apps/desktop/src-tauri/Cargo\.toml)$' })
    $inputBytes = [Text.Encoding]::UTF8.GetBytes(($inputs -join "`n"))
    return [ordered]@{
        head = "$head".Trim()
        dirty = $status.Count -gt 0
        status = $status
        sha256 = $sourceHash
        build_inputs_sha256 = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($inputBytes)).ToLowerInvariant()
    }
}

function Select-ProductionServerArtifact([string[]]$Messages, [string]$PackageId, [string]$ExpectedExecutable) {
    $artifacts = @()
    $finished = $false
    foreach ($line in $Messages) {
        if (-not $line.Trim()) { continue }
        $message = $line | ConvertFrom-Json
        if ($message.reason -eq 'build-finished') { $finished = $message.success; continue }
        if ($message.reason -ne 'compiler-artifact' -or $message.package_id -ne $PackageId) { continue }
        if (@($message.features).Count -ne 0) { throw 'Production Server must have no enabled features (including test-fixture)' }
        if (-not $message.executable) { continue }
        if ($message.target.name -ne 'latte-work-server' -or @($message.target.kind) -notcontains 'bin') {
            throw 'Unexpected executable in production Server build'
        }
        if ($message.profile.test -or $message.profile.debug_assertions -or $message.profile.opt_level -ne '3') {
            throw 'Server artifact is not a production release profile'
        }
        if ([IO.Path]::GetFullPath($message.executable) -ne [IO.Path]::GetFullPath($ExpectedExecutable)) {
            throw 'Cargo reported an unexpected Server executable path'
        }
        $artifacts += $message
    }
    if (-not $finished -or $artifacts.Count -ne 1) { throw 'Missing successful production Server compiler artifact' }
    $binary = Get-Item -LiteralPath $ExpectedExecutable
    if ($binary.Length -eq 0) { throw 'Server executable is empty' }
    return $artifacts[0]
}

function Invoke-ServerHelp([string]$Executable, [string]$Version) {
    $start = [Diagnostics.ProcessStartInfo]::new($Executable)
    $start.ArgumentList.Add('--help')
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $process = [Diagnostics.Process]::Start($start)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(10000)) {
            $process.Kill($true)
            throw 'Server --help timed out'
        }
        $output = $stdout.GetAwaiter().GetResult()
        $errorOutput = $stderr.GetAwaiter().GetResult()
        if ($process.ExitCode -ne 0 -or $errorOutput -or -not $output.StartsWith("latte-work-server $Version`n") -or $output -notmatch 'Usage: latte-work-server') {
            throw 'Server --help did not match the production CLI contract'
        }
        return $output.Trim()
    } finally { $process.Dispose() }
}

function Test-ServerRelease([string]$Directory) {
    $directoryPath = (Resolve-Path -LiteralPath $Directory).Path
    $names = @(Get-ChildItem -LiteralPath $directoryPath | Select-Object -ExpandProperty Name | Sort-Object)
    if (($names -join ',') -ne 'latte-work-server.exe,latte-work-server.exe.sha256,provenance.json') {
        throw 'Server artifact directory must contain exactly the executable, checksum and provenance'
    }
    $provenance = Get-Content -LiteralPath (Join-Path $directoryPath 'provenance.json') -Raw | ConvertFrom-Json
    if ($provenance.schema_version -ne 1 -or $provenance.kind -ne 'standalone-windows-server' -or
        $provenance.target -ne 'x86_64-pc-windows-msvc' -or $provenance.profile -ne 'release' -or
        @($provenance.features).Count -ne 0 -or $provenance.artifact.file -ne 'latte-work-server.exe' -or
        $provenance.source.head -notmatch '^[0-9a-f]{40}$' -or $provenance.source.sha256 -notmatch '^[0-9a-f]{64}$') {
        throw 'Invalid production Server provenance'
    }
    $executable = Join-Path $directoryPath $provenance.artifact.file
    $file = Get-Item -LiteralPath $executable
    $hash = (Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash.ToLowerInvariant()
    $checksum = (Get-Content -LiteralPath "$executable.sha256" -Raw).Trim()
    if ($file.Length -eq 0 -or $file.Length -ne $provenance.artifact.bytes -or $hash -ne $provenance.artifact.sha256 -or
        $checksum -cne "$hash  latte-work-server.exe") { throw 'Server checksum, size or provenance mismatch' }
    $help = Invoke-ServerHelp $executable $provenance.version
    if ($help -cne $provenance.smoke_help) { throw 'Server smoke output differs from provenance' }
    return $provenance
}

function Build-ServerRelease([string]$Directory) {
    if (-not $IsWindows) { throw 'This helper requires native Windows' }
    $root = Split-Path -Parent $PSScriptRoot
    Push-Location $root
    try {
        $rustc = (Invoke-ServerTool rustc @('-vV')) -join "`n"
        $triple = ([regex]::Match($rustc, '(?m)^host: (.+)$')).Groups[1].Value.Trim()
        if ($triple -ne 'x86_64-pc-windows-msvc') { throw "Unsupported Windows release target: $triple" }
        $metadata = ((Invoke-ServerTool cargo @('metadata', '--no-deps', '--format-version=1', '--locked')) -join "`n") | ConvertFrom-Json
        $package = @($metadata.packages | Where-Object name -eq 'latte-work-server')
        if ($package.Count -ne 1) { throw 'Cannot identify Server package' }
        $buildTarget = Join-Path $metadata.target_directory 'windows-server-production'
        $runId = [guid]::NewGuid().ToString('N')
        if (-not $Directory) { $Directory = Join-Path $metadata.target_directory "windows-server/$runId" }
        $Directory = [IO.Path]::GetFullPath($Directory)
        if (Test-Path -LiteralPath $Directory) { throw 'Refusing an existing artifact directory; use a fresh output path' }
        $logDirectory = Join-Path $metadata.target_directory "windows-server-build/$runId"
        New-Item -ItemType Directory -Path $logDirectory -Force | Out-Null
        $messagesPath = Join-Path $logDirectory 'cargo.jsonl'
        $source = Get-ServerSourceIdentity $root
        $arguments = @('build', '--release', '-p', 'latte-work-server', '--locked', '--message-format=json', '--target-dir', $buildTarget, '--target', $triple)
        & cargo @arguments > $messagesPath
        if ($LASTEXITCODE -ne 0) { throw "Production Server build failed with exit code $LASTEXITCODE" }
        $expectedExecutable = Join-Path $buildTarget "$triple/release/latte-work-server.exe"
        $artifact = Select-ProductionServerArtifact @(Get-Content -LiteralPath $messagesPath) $package[0].id $expectedExecutable
        $after = Get-ServerSourceIdentity $root
        if ($source.head -ne $after.head -or $source.build_inputs_sha256 -ne $after.build_inputs_sha256) {
            throw 'Server build inputs changed during compilation; rerun on stable sources'
        }
        $help = Invoke-ServerHelp $artifact.executable $package[0].version
        $hash = (Get-FileHash -LiteralPath $artifact.executable -Algorithm SHA256).Hash.ToLowerInvariant()
        New-Item -ItemType Directory -Path $Directory | Out-Null
        $destination = Join-Path $Directory 'latte-work-server.exe'
        Copy-Item -LiteralPath $artifact.executable -Destination $destination
        Set-Content -LiteralPath "$destination.sha256" -Value "$hash  latte-work-server.exe" -Encoding utf8NoBOM
        $provenance = [ordered]@{
            schema_version = 1
            kind = 'standalone-windows-server'
            version = $package[0].version
            target = $triple
            profile = 'release'
            features = @($artifact.features)
            source = $source
            tools = [ordered]@{ rustc = $rustc; cargo = (Invoke-ServerTool cargo @('--version')) -join "`n" }
            command = @('cargo') + $arguments
            compiler_executable = $artifact.executable
            artifact = [ordered]@{ file = 'latte-work-server.exe'; bytes = (Get-Item -LiteralPath $destination).Length; sha256 = $hash }
            smoke_help = $help
            built_at_utc = [DateTime]::UtcNow.ToString('o')
        }
        $provenance | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $Directory 'provenance.json') -Encoding utf8NoBOM
        Test-ServerRelease $Directory | Out-Null
        if ($env:GITHUB_OUTPUT) { Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "artifact-directory=$Directory" -Encoding utf8NoBOM }
        Write-Host "Production Server artifact: $Directory"
        Write-Host "SHA256: $hash"
    } finally { Pop-Location }
}

if ($MyInvocation.InvocationName -ne '.') {
    if ($Mode -eq 'Verify') {
        if (-not $ArtifactDirectory) { throw 'Verify requires -ArtifactDirectory' }
        Test-ServerRelease $ArtifactDirectory
    } else { Build-ServerRelease $ArtifactDirectory }
}
