# Hosting smoke under WinSW (run by hosting/winsw on a CI Windows runner; it registers a Windows
# service, so it refuses to run outside CI). WinSW v2.12.0, the version the deployment packages,
# wraps the smoke binary with the deployment's settings: rotate log mode, onfailure restart after
# 10 s and 20 s, no stoptimeout (WinSW's default is 15 s). For exit codes 75 and 70 it asserts that
# the failed run is restarted by onfailure, and that `stop` delivers CTRL_C and the service stops
# gracefully with exit code 0 within 15 s.
# Usage: scripts/hosting/winsw.ps1 -Binary <path to smoke.exe>
param([Parameter(Mandatory)][string]$Binary)
$ErrorActionPreference = 'Stop'
if ($env:CI -ne 'true') { throw 'winsw.ps1 registers a Windows service; it only runs in CI (CI=true)' }

$url = 'https://github.com/winsw/winsw/releases/download/v2.12.0/WinSW-x64.exe'
$sha256 = '05B82D46AD331CC16BDC00DE5C6332C1EF818DF8CEEFCD49C726553209B3A0DA'
$id = 'rivium-smoke'
$script:failures = 0

function Check([string]$Name, [bool]$Ok) {
    if ($Ok) { Write-Output "ok   $Name" } else { Write-Output "FAIL $Name"; $script:failures++ }
}

function Wait-Until([int]$Seconds, [scriptblock]$Condition) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    while (-not (& $Condition)) {
        if ((Get-Date) -gt $deadline) { return $false }
        Start-Sleep -Milliseconds 200
    }
    return $true
}

function Count([string]$Pattern, [string]$Path) {
    if (-not (Test-Path $Path)) { return 0 }
    return @(Select-String -Path $Path -Pattern $Pattern -SimpleMatch).Count
}

function Test-Scenario([int]$Code) {
    Write-Output "--- scenario: first run exits with $Code"
    $dir = Join-Path $env:RUNNER_TEMP "winsw-smoke-$Code"
    New-Item -ItemType Directory -Force $dir | Out-Null
    $winsw = Join-Path $dir 'smoke-service.exe'
    Invoke-WebRequest $url -OutFile $winsw
    if ((Get-FileHash $winsw -Algorithm SHA256).Hash -ne $sha256) { throw 'WinSW checksum mismatch' }
    Copy-Item $Binary (Join-Path $dir 'smoke.exe')
    @"
<service>
  <id>$id</id>
  <name>Rivium hosting smoke</name>
  <description>Rivium hosting smoke test</description>
  <executable>%BASE%\smoke.exe</executable>
  <arguments>serve --log "%BASE%\smoke.log" --exit-once $Code --marker "%BASE%\exited"</arguments>
  <workingdirectory>%BASE%</workingdirectory>
  <logmode>rotate</logmode>
  <logpath>%BASE%\logs</logpath>
  <onfailure action="restart" delay="10 sec"/>
  <onfailure action="restart" delay="20 sec"/>
</service>
"@ | Set-Content -Encoding UTF8 (Join-Path $dir 'smoke-service.xml')

    $log = Join-Path $dir 'smoke.log'
    $wrapper = Join-Path $dir 'logs\smoke-service.wrapper.log'
    & $winsw install
    try {
        & $winsw start
        # The first run exits with $Code; onfailure restarts it after 10 s.
        Check "exit $Code is restarted by onfailure" (Wait-Until 60 { (Count ' ready' $log) -ge 1 })
        Check 'two runs started' ((Count 'started' $log) -eq 2)

        $watch = [Diagnostics.Stopwatch]::StartNew()
        & $winsw stop
        $stopped = Wait-Until 30 { (Get-Service $id).Status -eq 'Stopped' }
        $watch.Stop()
        Write-Output "stop took $($watch.ElapsedMilliseconds) ms"
        Check 'service stopped' $stopped
        Check 'stopped within 15000 ms' ($watch.ElapsedMilliseconds -lt 15000)
        Check 'CTRL_C received and graceful stop logged' (((Count 'stop signal=CTRL_C' $log) -eq 1) -and ((Count ' stopped' $log) -eq 1))
        Write-Output '--- smoke log'; Get-Content $log
        Write-Output '--- WinSW log'; Get-Content $wrapper -ErrorAction SilentlyContinue
        Check 'WinSW recorded exit code 0 for the stopped run' ((Count 'finished with 0' $wrapper) -ge 1)
    } finally {
        & $winsw stop 2>$null | Out-Null
        & $winsw uninstall
    }
}

Test-Scenario 75
Test-Scenario 70
if ($script:failures -ne 0) { Write-Output "hosting/winsw: $($script:failures) failure(s)"; exit 1 }
Write-Output 'hosting/winsw: passed'
