# Hosting test under WinSW (run by hosting/winsw on a CI Windows runner; it registers a Windows
# service, so it refuses to run outside CI). WinSW v2.12.0, the version the deployment packages,
# wraps the program with the deployment's settings: rotate log mode, onfailure restart after 10 s
# and 20 s, no stoptimeout (WinSW's default is 15 s), no arguments. The program runs from its
# directory, which is its root, next to a config.toml that only sets its address. Scenarios:
#   restart  -Binary: a restart request exits with 75 and onfailure restarts the service. udp-echo
#            is asked with the datagram `restart`; edge-lite with a new configuration through its
#            API, which injects unit panics that the restarted run must keep running through;
#   faults   -Faults, udp-echo-faults: the first three runs fail with 70 once ready, and each
#            failure is restarted, the third one by the last onfailure action again.
# In both, `stop` then delivers CTRL_C and the service stops gracefully with exit code 0 within
# 15 s, without a forced kill.
# Usage: scripts/hosting/winsw.ps1 -Binary <udp-echo.exe or edge-lite.exe> [-Faults <udp-echo-faults.exe>]
#        [-Scenarios restart,faults] [-ExtraArgs '<run arguments>']
param(
    [Parameter(Mandatory)][string]$Binary,
    [string]$Faults = '',
    [string[]]$Scenarios = @('restart', 'faults'),
    [string]$ExtraArgs = ''
)
$ErrorActionPreference = 'Stop'
if ($env:CI -ne 'true') { throw 'winsw.ps1 registers a Windows service; it only runs in CI (CI=true)' }

$url = 'https://github.com/winsw/winsw/releases/download/v2.12.0/WinSW-x64.exe'
$sha256 = '05B82D46AD331CC16BDC00DE5C6332C1EF818DF8CEEFCD49C726553209B3A0DA'
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

function Send-Datagram([string]$Text, [int]$Port) {
    $udp = New-Object System.Net.Sockets.UdpClient
    try {
        $bytes = [Text.Encoding]::ASCII.GetBytes($Text)
        $null = $udp.Send($bytes, $bytes.Length, '127.0.0.1', $Port)
    } finally { $udp.Dispose() }
}

function Test-Scenario([string]$Scenario) {
    Write-Output "--- scenario: $Scenario"
    $source = if ($Scenario -eq 'faults') { $Faults } else { $Binary }
    if (-not $source) { throw "the $Scenario scenario needs its program" }
    $name = [IO.Path]::GetFileNameWithoutExtension($source)
    $id = "rivium-$name"
    $port = 7357
    $dir = Join-Path $env:RUNNER_TEMP "winsw-$Scenario"
    Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $dir | Out-Null
    $winsw = Join-Path $dir "$name-service.exe"
    Invoke-WebRequest $url -OutFile $winsw
    if ((Get-FileHash $winsw -Algorithm SHA256).Hash -ne $sha256) { throw 'WinSW checksum mismatch' }
    Copy-Item $source (Join-Path $dir "$name.exe")
    $section = if ($name -eq 'edge-lite') { 'http' } else { 'echo' }
    $config = "[$section]`naddr = `"127.0.0.1:$port`"`n"
    if ($Scenario -eq 'faults') { $config += "`n[faults]`nfail_after_ms = 500`nfail_runs = 3`n" }
    [IO.File]::WriteAllText((Join-Path $dir 'config.toml'), $config)
    $arguments = if ($ExtraArgs) { "`n  <arguments>$ExtraArgs</arguments>" } else { '' }
    @"
<service>
  <id>$id</id>
  <name>Rivium hosting test ($name)</name>
  <description>Rivium hosting test</description>
  <executable>%BASE%\$name.exe</executable>$arguments
  <workingdirectory>%BASE%</workingdirectory>
  <logmode>rotate</logmode>
  <logpath>%BASE%\logs</logpath>
  <onfailure action="restart" delay="10 sec"/>
  <onfailure action="restart" delay="20 sec"/>
</service>
"@ | Set-Content -Encoding UTF8 (Join-Path $dir "$name-service.xml")

    $log = Join-Path $dir "logs\$name\$name.log"
    $wrapper = Join-Path $dir "logs\$name-service.wrapper.log"
    $running = 'phase changed phase="running"'
    & $winsw install
    try {
        & $winsw start
        if ($Scenario -eq 'faults') {
            # Runs 1 to 3 fail once ready; onfailure restarts them after 10, 20 and 20 s.
            Check 'four runs, the last one running' (Wait-Until 120 { (Count $running $log) -ge 4 })
            Check 'three runs failed with 70' ((Count 'stopped code="Fault" exit_code=70' $log) -eq 3)
        } else {
            Check 'first run is running' (Wait-Until 30 { (Count $running $log) -ge 1 })
            if ($name -eq 'edge-lite') {
                # A new configuration through the API, which turns on injected unit panics.
                $new = $config + "`n[faults]`npanic_every = 2`n"
                $reply = Invoke-WebRequest -Method Put -Uri "http://127.0.0.1:$port/api/config" -Body $new -UseBasicParsing
                Check 'the API accepted the new configuration' ($reply.Content -match '"status":"success"')
            } else {
                Send-Datagram 'restart' $port
            }
            # The run exits with 75; onfailure restarts it after 10 s.
            Check 'exit 75 is restarted by onfailure' (Wait-Until 60 { (Count $running $log) -ge 2 })
            Check 'the restart request is logged' ((Count 'stopped code="Restart" exit_code=75' $log) -eq 1)
            if ($name -eq 'edge-lite') {
                $isolated = 'a unit failed: it starts again unit=[0-9]+ failures=[0-9]+ why="panic: a panic injected'
                Check 'the restarted run isolates unit panics' (Wait-Until 30 { @(Select-String -Path $log -Pattern $isolated).Count -ge 1 })
                Start-Sleep -Seconds 2
                Check 'the panics do not end the run' ((Count $running $log) -eq 2)
            }
        }

        # WinSW 2.12 does not log the child's exit code, so hold a handle to the running child and
        # read the exit code from it once the child has exited.
        $child = Get-Process -Name $name | Select-Object -First 1
        $null = $child.Handle
        $watch = [Diagnostics.Stopwatch]::StartNew()
        & $winsw stop
        $stopped = Wait-Until 30 { (Get-Service $id).Status -eq 'Stopped' }
        $watch.Stop()
        Write-Output "stop took $($watch.ElapsedMilliseconds) ms"
        Check 'service stopped' $stopped
        Check 'stopped within 15000 ms' ($watch.ElapsedMilliseconds -lt 15000)
        Check 'CTRL_C received and graceful stop logged' (((Count 'phase="stopping" reason=CTRL_C' $log) -eq 1) -and ((Count 'stopped code="Ok" exit_code=0' $log) -eq 1))
        Check 'stopped run exited with code 0' ($child.WaitForExit(15000) -and $child.ExitCode -eq 0)
        Write-Output "stopped run exit code: $($child.ExitCode)"
        # WinSW 2.12 logs "Process <pid> terminated." when CTRL_C did not stop the child in time.
        Check 'no forced kill' ((Count "Process $($child.Id) terminated." $wrapper) -eq 0)
        Write-Output "--- $name log"; Get-Content $log
        Write-Output '--- WinSW log'; Get-Content $wrapper -ErrorAction SilentlyContinue
    } finally {
        & $winsw stop 2>$null | Out-Null
        & $winsw uninstall
    }
}

foreach ($scenario in $Scenarios) { Test-Scenario $scenario }
if ($script:failures -ne 0) { Write-Output "hosting/winsw: $($script:failures) failure(s)"; exit 1 }
Write-Output 'hosting/winsw: passed'
