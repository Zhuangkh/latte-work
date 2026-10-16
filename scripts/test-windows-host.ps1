# Build as the runner; Host checks elevated same-user behavior before the ordinary-user suite.
# Account provisioning is restricted to disposable GitHub Actions Windows runners.
[CmdletBinding()]
param(
    [ValidateSet('Host', 'Desktop')]
    [string]$Suite = 'Host'
)
$ErrorActionPreference = 'Stop'
$cargoArgs = @('test', '--no-run', '--lib', '--bins', '--tests', '--locked', '--message-format=json')
if ($Suite -eq 'Host') {
    $packages = @('latte-work-windows-security', 'latte-work-platform', 'latte-work-config', 'latte-work-protocol', 'latte-work-client', 'latte-work-server')
    $cargoArgs += @('--features', 'latte-work-server/test-fixture')
} else {
    $packages = @('latte-work-desktop', 'latte-work-windows-webview')
}
foreach ($package in $packages) { $cargoArgs += @('-p', $package) }
$artifacts = & cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { throw 'Cannot build native test executables' }
$executables = @($artifacts | ForEach-Object {
    $artifact = $_ | ConvertFrom-Json
    if ($artifact.reason -eq 'compiler-artifact' -and $artifact.profile.test -and $artifact.executable) {
        $artifact.executable
    }
} | Sort-Object -Unique)
if ($executables.Count -eq 0) { throw 'No test executables were built' }
foreach ($executable in $executables) {
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) { throw "Missing test executable: $executable" }
}
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    foreach ($executable in $executables) {
        & $executable --test-threads=1
        if ($LASTEXITCODE -ne 0) { throw "Test failed: $executable" }
    }
    exit 0
}
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') {
    throw 'Run this ordinary-user suite locally without elevation; use test-windows-elevated.ps1 for elevated acceptance. Test-account provisioning is CI-only'
}
if ($Suite -eq 'Host') {
    # Managed-root fixtures reject short aliases in every ancestor. Expand only
    # this harness-owned directory, never paths supplied by the security tests.
    $elevatedTemp = Join-Path $env:RUNNER_TEMP ('latte-elevated-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $elevatedTemp | Out-Null
    $elevatedTemp = (Get-Item -LiteralPath $elevatedTemp).FullName
    $tempAcl = [Security.AccessControl.DirectorySecurity]::new()
    $tempAcl.SetOwner($identity.User)
    $tempAcl.SetAccessRuleProtection($true, $false)
    $tempAcl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($identity.User, 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow'))
    Set-Acl -LiteralPath $elevatedTemp -AclObject $tempAcl
    $originalTemp = $env:TEMP
    $originalTmp = $env:TMP
    try {
        $env:TEMP = $elevatedTemp
        $env:TMP = $elevatedTemp
        Write-Host "Elevated fixture TEMP/TMP: $elevatedTemp"
        & (Join-Path $PSScriptRoot 'test-windows-elevated.ps1')
        if ($LASTEXITCODE -ne 0) { throw 'Elevated same-user acceptance failed' }
    } finally {
        $env:TEMP = $originalTemp
        $env:TMP = $originalTmp
        # Runner teardown owns this fixture directory, like ordinary-user logs.
    }
}

$userName = 'latte-' + [Guid]::NewGuid().ToString('N').Substring(0, 12)
$password = ConvertTo-SecureString ('Aa1!' + [Guid]::NewGuid().ToString('N')) -AsPlainText -Force
$work = Join-Path $env:RUNNER_TEMP $userName
$workspaceAcl = Get-Acl $env:GITHUB_WORKSPACE
$runnerTempAcl = Get-Acl $env:RUNNER_TEMP
$userCreated = $false
try {
    $user = New-LocalUser -Name $userName -Password $password -Description 'Ephemeral Latte native test user'
    $userCreated = $true
    # A new local user is never added to Administrators.
    $usersGroup = Get-LocalGroup -SID 'S-1-5-32-545'
    if (-not (Get-LocalGroupMember -Group $usersGroup | Where-Object { $_.SID -eq $user.SID })) {
        Add-LocalGroupMember -Group $usersGroup -Member $user
    }
    New-Item -ItemType Directory -Path $work | Out-Null
    foreach ($entry in @(@($work, 'FullControl'), @($env:GITHUB_WORKSPACE, 'ReadAndExecute'), @($env:RUNNER_TEMP, 'ReadAndExecute'))) {
        $acl = Get-Acl $entry[0]
        $rule = [Security.AccessControl.FileSystemAccessRule]::new($user.SID, $entry[1], 'ContainerInherit,ObjectInherit', 'None', 'Allow')
        $acl.AddAccessRule($rule)
        Set-Acl -Path $entry[0] -AclObject $acl
    }
    $executables | ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $work 'tests.json')
    @'
$ErrorActionPreference = 'Stop'
$env:TEMP = Join-Path $PSScriptRoot 'temp'
$env:TMP = $env:TEMP
New-Item -ItemType Directory -Path $env:TEMP | Out-Null
$env:GIT_CONFIG_GLOBAL = Join-Path $PSScriptRoot 'empty.gitconfig'
New-Item -ItemType File -Path $env:GIT_CONFIG_GLOBAL | Out-Null
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Native tests require a non-elevated token' }
foreach ($executable in (Get-Content -Raw (Join-Path $PSScriptRoot 'tests.json') | ConvertFrom-Json)) {
    & $executable --test-threads=1
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
exit 0
'@ | Set-Content -Encoding UTF8 (Join-Path $work 'run.ps1')
    $credential = [System.Management.Automation.PSCredential]::new("$env:COMPUTERNAME\$userName", $password)
    $arguments = @('-NoProfile', '-NonInteractive', '-File', ('"' + (Join-Path $work 'run.ps1') + '"'))
    $child = Start-Process -FilePath (Get-Command powershell.exe).Source -ArgumentList $arguments -Credential $credential -LoadUserProfile -WindowStyle Hidden -WorkingDirectory $work -PassThru -Wait -RedirectStandardOutput (Join-Path $work 'stdout.log') -RedirectStandardError (Join-Path $work 'stderr.log')
    Get-Content (Join-Path $work 'stdout.log')
    Get-Content (Join-Path $work 'stderr.log')
    if ($child.ExitCode -ne 0) { throw "Native tests failed with exit code $($child.ExitCode)" }
} finally {
    Set-Acl -Path $env:GITHUB_WORKSPACE -AclObject $workspaceAcl
    Set-Acl -Path $env:RUNNER_TEMP -AclObject $runnerTempAcl
    if ($userCreated) { Remove-LocalUser -Name $userName }
    # Keep logs and the disposable profile for CI diagnostics; runner teardown owns cleanup.
}
