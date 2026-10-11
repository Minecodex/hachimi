$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
if ($env:GITHUB_ACTIONS -ne "true") {
    throw "Disposable desktop CI users can only be created on a GitHub Actions runner"
}

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$nodeExecutable = (Get-Command node.exe -ErrorAction Stop).Source
$powerShell = Join-Path $env:SystemRoot "System32/WindowsPowerShell/v1.0/powershell.exe"
$targetRoot = Join-Path $repositoryRoot "target"
$workParent = [IO.Path]::GetFullPath((Join-Path $env:RUNNER_TEMP "HachimiDesktopCi"))
$suffix = [Guid]::NewGuid().ToString("N")
$workRoot = [IO.Path]::GetFullPath((Join-Path $workParent $suffix))
$workPrefix = $workParent.TrimEnd('\') + '\'
if (-not $workRoot.StartsWith($workPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "desktop_ci_work_root_invalid"
}
$userName = "hachi-ui-$($suffix.Substring(0, 8))"
$account = "$env:COMPUTERNAME\$userName"
$password = ConvertTo-SecureString "Hachimi!$([Guid]::NewGuid().ToString('N'))aA1" -AsPlainText -Force
$credential = [pscredential]::new($account, $password)
$userCreated = $false
$desktopGrep = if ($env:HACHIMI_DESKTOP_E2E_GREP) { $env:HACHIMI_DESKTOP_E2E_GREP } else { ".*" }
$grepBase64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($desktopGrep))
$desktopProfile = if ($env:HACHIMI_DESKTOP_E2E_PROFILE) { $env:HACHIMI_DESKTOP_E2E_PROFILE } else { "release" }
if ($desktopProfile -notin @("pr", "release")) { throw "desktop_ci_profile_invalid" }

try {
    Push-Location $repositoryRoot
    try {
        & $nodeExecutable "scripts/windows-only.mjs" "scripts/prepare-desktop-e2e-tools.ps1"
        if ($LASTEXITCODE -ne 0) { throw "desktop_ci_driver_prepare_failed" }
        & $nodeExecutable "scripts/desktop-e2e/support/build.mjs"
        if ($LASTEXITCODE -ne 0) { throw "desktop_ci_build_failed" }
    } finally { Pop-Location }

    New-LocalUser -Name $userName -Password $password -PasswordNeverExpires -AccountNeverExpires | Out-Null
    $userCreated = $true
    New-Item -ItemType Directory -Path $workRoot -Force | Out-Null
    foreach ($grant in @(
        @{ Path = $repositoryRoot; Rights = "RX" },
        @{ Path = $targetRoot; Rights = "M" },
        @{ Path = $workRoot; Rights = "M" }
    )) {
        icacls.exe $grant.Path /grant "${account}:(OI)(CI)$($grant.Rights)" /T /Q | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "desktop_ci_fixture_access_failed" }
    }

    $innerScript = Join-Path $workRoot "run-desktop.ps1"
    @'
param([string]$RepositoryRoot, [string]$NodeExecutable, [string]$WorkRoot, [string]$GrepBase64, [ValidateSet("pr", "release")][string]$DesktopProfile)
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Desktop E2E unexpectedly ran with administrator rights"
}
$env:USERPROFILE = [Environment]::GetFolderPath('UserProfile')
$env:TEMP = Join-Path $WorkRoot "temp"
$env:TMP = $env:TEMP
New-Item -ItemType Directory -Path $env:TEMP -Force | Out-Null
$env:HACHIMI_DESKTOP_E2E_PREBUILT = "1"
$env:HACHIMI_DESKTOP_E2E_GREP = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($GrepBase64))
$env:HACHIMI_DESKTOP_E2E_PROFILE = $DesktopProfile
Set-Location -LiteralPath $RepositoryRoot
Write-Output "Desktop E2E identity: standard user; administrator=false"
Write-Output "Desktop E2E selection: $env:HACHIMI_DESKTOP_E2E_GREP"
Write-Output "Desktop E2E profile: $env:HACHIMI_DESKTOP_E2E_PROFILE"
& $NodeExecutable "scripts/desktop-e2e/run.mjs"
exit $LASTEXITCODE
'@ | Set-Content -LiteralPath $innerScript -Encoding UTF8

    $stdoutPath = Join-Path $workRoot "stdout.log"
    $stderrPath = Join-Path $workRoot "stderr.log"
    $process = Start-Process -FilePath $powerShell -Credential $credential -LoadUserProfile `
        -ArgumentList @("-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
            "-File", $innerScript, "-RepositoryRoot", $repositoryRoot,
            "-NodeExecutable", $nodeExecutable, "-WorkRoot", $workRoot, "-GrepBase64", $grepBase64, "-DesktopProfile", $desktopProfile) `
        -WorkingDirectory $repositoryRoot -WindowStyle Hidden `
        -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath -Wait -PassThru
    if (Test-Path -LiteralPath $stdoutPath) { Get-Content -LiteralPath $stdoutPath }
    if (Test-Path -LiteralPath $stderrPath) { Get-Content -LiteralPath $stderrPath | ForEach-Object { Write-Host $_ } }
    if ($process.ExitCode -ne 0) { throw "Standard-user desktop E2E failed with $($process.ExitCode)" }
} finally {
    if ($userCreated) { Remove-LocalUser -Name $userName -ErrorAction SilentlyContinue }
    if (Test-Path -LiteralPath $workRoot) {
        $resolvedWorkRoot = (Resolve-Path -LiteralPath $workRoot).Path
        if ($resolvedWorkRoot.StartsWith($workPrefix, [StringComparison]::OrdinalIgnoreCase)) {
            Remove-Item -LiteralPath $resolvedWorkRoot -Recurse -Force
        }
    }
}
