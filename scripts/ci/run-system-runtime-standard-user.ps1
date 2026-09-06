$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
Push-Location $repositoryRoot
try {
    cargo test -p hachimi-system-runtime --no-run
    if ($LASTEXITCODE -ne 0) {
        throw "hachimi-system-runtime test binary build failed with exit code $LASTEXITCODE"
    }

    $testExecutable = Get-ChildItem (Join-Path $repositoryRoot "target/debug/deps/hachimi_system_runtime-*.exe") |
        Where-Object { $_.Extension -eq ".exe" } |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1
    if ($null -eq $testExecutable) {
        throw "hachimi-system-runtime test executable was not produced"
    }

    $suffix = [Guid]::NewGuid().ToString("N").Substring(0, 10)
    $userName = "hachi-$suffix"
    $plainPassword = "Hachimi!$([Guid]::NewGuid().ToString('N'))aA1"
    $securePassword = ConvertTo-SecureString $plainPassword -AsPlainText -Force
    $credential = [pscredential]::new("$env:COMPUTERNAME\$userName", $securePassword)
    $workRoot = Join-Path $env:ProgramData "HachimiCi/$suffix"
    $innerScript = Join-Path $workRoot "run-runtime-test.ps1"
    $copiedTest = Join-Path $workRoot $testExecutable.Name
    $stdoutPath = Join-Path $workRoot "stdout.log"
    $stderrPath = Join-Path $workRoot "stderr.log"
    $userCreated = $false

    try {
        New-LocalUser -Name $userName -Password $securePassword -PasswordNeverExpires -AccountNeverExpires |
            Out-Null
        $userCreated = $true
        New-Item -ItemType Directory -Path $workRoot -Force | Out-Null
        Copy-Item $testExecutable.FullName $copiedTest
        icacls.exe $workRoot /inheritance:e /grant "${userName}:(OI)(CI)M" /T /Q | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "failed to grant the disposable standard user access to $workRoot"
        }

        @'
param([Parameter(Mandatory = $true)][string]$TestExecutable)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "system-runtime discovery test unexpectedly ran with administrator rights"
}

$testRoot = Split-Path -Parent $TestExecutable
$env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
$env:TEMP = Join-Path $testRoot "temp"
$env:TMP = $env:TEMP
$env:HACHIMI_REQUIRE_STANDARD_GIT_DISCOVERY = "1"
New-Item -ItemType Directory -Path $env:TEMP -Force | Out-Null

& $TestExecutable "tests::windows_registry_or_standard_install_is_found_without_process_path" "--exact" "--test-threads=1"
exit $LASTEXITCODE
'@ | Set-Content -LiteralPath $innerScript -Encoding UTF8

        $powerShell = Join-Path $env:SystemRoot "System32/WindowsPowerShell/v1.0/powershell.exe"
        $process = Start-Process -FilePath $powerShell `
            -Credential $credential `
            -ArgumentList @(
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy", "Bypass",
                "-File", $innerScript,
                "-TestExecutable", $copiedTest
            ) `
            -WorkingDirectory $workRoot `
            -RedirectStandardOutput $stdoutPath `
            -RedirectStandardError $stderrPath `
            -Wait `
            -PassThru
        if (Test-Path $stdoutPath) {
            Get-Content $stdoutPath
        }
        if (Test-Path $stderrPath) {
            Get-Content $stderrPath | ForEach-Object { Write-Host $_ }
        }
        if ($process.ExitCode -ne 0) {
            throw "standard-user system-runtime discovery failed with exit code $($process.ExitCode)"
        }
    }
    finally {
        if ($userCreated) {
            Remove-LocalUser -Name $userName -ErrorAction SilentlyContinue
        }
        if (Test-Path $workRoot) {
            Remove-Item -LiteralPath $workRoot -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}
finally {
    Pop-Location
}
