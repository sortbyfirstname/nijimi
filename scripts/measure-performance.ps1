<#
.SYNOPSIS
    Measures nijimi's CPU, GPU, and memory cost while a configured game is running.

.DESCRIPTION
    Start nijimi (ideally a full `cargo build --release` build) and launch a game that has
    a profile, then run this script and switch to the game during the countdown. Keep the game in
    the foreground and play normally until sampling finishes.

    Each second it records which app is in the foreground, whether the ambient overlay is visible,
    nijimi's CPU, GPU-engine and memory use, and the foreground game's own GPU 3D use.
    Statistics are computed only over seconds where the overlay was active.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\measure-performance.ps1
    powershell -ExecutionPolicy Bypass -File scripts\measure-performance.ps1 -Seconds 120 -Delay 15
#>
param(
    [int]$Seconds = 60,
    [int]$Delay = 10
)

$ErrorActionPreference = 'Stop'

Add-Type -Namespace Nijimi -Name Win32 -MemberDefinition @'
[DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, IntPtr title);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
'@

function Get-ForegroundPid {
    $fgPid = [uint32]0
    [void][Nijimi.Win32]::GetWindowThreadProcessId([Nijimi.Win32]::GetForegroundWindow(), [ref]$fgPid)
    return [int]$fgPid
}

function Test-OverlayVisible {
    # Title is an IntPtr so we can pass a real NULL; PowerShell turns $null strings into "".
    $hwnd = [Nijimi.Win32]::FindWindow('NijimiOverlayWindow', [IntPtr]::Zero)
    return ($hwnd -ne [IntPtr]::Zero) -and [Nijimi.Win32]::IsWindowVisible($hwnd)
}

function Get-PrivateWorkingSetMB([int]$processId) {
    $perf = Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter "IDProcess=$processId"
    if ($perf) { return [double]$perf.WorkingSetPrivate / 1MB }
    return [double]::NaN
}

function Get-Stats([double[]]$values) {
    if (-not $values -or $values.Count -eq 0) { return 'n/a' }
    $sorted = $values | Sort-Object
    $avg = ($values | Measure-Object -Average).Average
    $p95 = $sorted[[math]::Min($sorted.Count - 1, [math]::Ceiling($sorted.Count * 0.95) - 1)]
    $max = $sorted[-1]
    return ('avg {0:N2}  p95 {1:N2}  max {2:N2}' -f $avg, $p95, $max)
}

# The tray process, not a Settings window (which runs the same exe with --settings).
$trayProcess = Get-CimInstance Win32_Process -Filter "Name='nijimi.exe'" |
    Where-Object { $_.CommandLine -notmatch '--settings' } |
    Select-Object -First 1
if (-not $trayProcess) {
    Write-Error 'nijimi is not running. Start it, launch a configured game, then run this script again.'
}
$appPid = [int]$trayProcess.ProcessId
$app = Get-Process -Id $appPid
$cores = (Get-CimInstance Win32_Processor | Measure-Object NumberOfLogicalProcessors -Sum).Sum
$gpuName = (Get-CimInstance Win32_VideoController | Where-Object { $_.CurrentHorizontalResolution } |
    Select-Object -First 1)
$cpuName = (Get-CimInstance Win32_Processor | Select-Object -First 1).Name.Trim()

Write-Host "nijimi pid $appPid  ($($trayProcess.ExecutablePath))"
if ($trayProcess.ExecutablePath -notmatch '\\release\\') {
    Write-Warning 'This is not a target\release build; numbers may not represent a shipped build.'
}
Write-Host ''
for ($i = $Delay; $i -gt 0; $i--) {
    Write-Host -NoNewline "`rSwitch to your game now. Sampling starts in $i s...   "
    Start-Sleep -Seconds 1
}
Write-Host "`rSampling for $Seconds s. Keep the game in the foreground and play normally.        "

$samples = New-Object System.Collections.Generic.List[object]
$app.Refresh()
$lastCpu = $app.TotalProcessorTime
$lastTime = Get-Date

for ($second = 1; $second -le $Seconds; $second++) {
    $tickStart = Get-Date
    $fgPid = Get-ForegroundPid
    $fgName = (Get-Process -Id $fgPid -ErrorAction SilentlyContinue).ProcessName
    $overlay = Test-OverlayVisible

    $counters = (Get-Counter -ErrorAction SilentlyContinue -Counter @(
        "\GPU Engine(pid_$($appPid)_*)\Utilization Percentage",
        "\GPU Engine(pid_$($fgPid)_*engtype_3D)\Utilization Percentage",
        "\GPU Process Memory(pid_$($appPid)_*)\Dedicated Usage"
    )).CounterSamples

    $app3d = 0.0; $appCopy = 0.0; $appOther = 0.0; $game3d = 0.0; $appVramMB = 0.0
    foreach ($sample in $counters) {
        $path = $sample.Path
        if ($path -like '*gpu process memory*') {
            $appVramMB += $sample.CookedValue / 1MB
        } elseif ($path -like "*pid_$($appPid)_*") {
            if ($path -like '*engtype_3d*') { $app3d += $sample.CookedValue }
            elseif ($path -like '*engtype_copy*') { $appCopy += $sample.CookedValue }
            else { $appOther += $sample.CookedValue }
        } elseif ($path -like "*pid_$($fgPid)_*") {
            $game3d += $sample.CookedValue
        }
    }

    $app.Refresh()
    $now = Get-Date
    $cpuMs = ($app.TotalProcessorTime - $lastCpu).TotalMilliseconds
    $wallMs = ($now - $lastTime).TotalMilliseconds
    $lastCpu = $app.TotalProcessorTime
    $lastTime = $now

    $samples.Add([pscustomobject]@{
        Second          = $second
        Foreground      = $fgName
        OverlayActive   = $overlay
        AppCpuPctOfCore = [math]::Round($cpuMs / $wallMs * 100, 3)
        AppCpuPctTotal  = [math]::Round($cpuMs / $wallMs * 100 / $cores, 4)
        AppGpu3DPct     = [math]::Round($app3d, 3)
        AppGpuCopyPct   = [math]::Round($appCopy, 3)
        AppGpuOtherPct  = [math]::Round($appOther, 3)
        AppVramMB       = [math]::Round($appVramMB, 1)
        AppPrivateMB    = [math]::Round((Get-PrivateWorkingSetMB $appPid), 2)
        Game3DPct       = [math]::Round($game3d, 2)
    })

    Write-Host -NoNewline ("`r{0,3}/{1}  foreground: {2,-24} overlay: {3,-5}" -f $second, $Seconds, $fgName, $overlay)
    $remaining = 1000 - ((Get-Date) - $tickStart).TotalMilliseconds
    if ($remaining -gt 0) { Start-Sleep -Milliseconds $remaining }
}
Write-Host ''

$csvPath = Join-Path $env:TEMP ('nijimi-perf-{0:yyyyMMdd-HHmmss}.csv' -f (Get-Date))
$samples | Export-Csv -NoTypeInformation -Path $csvPath

$active = @($samples | Where-Object { $_.OverlayActive })
$games = ($active | Group-Object Foreground | Sort-Object Count -Descending | ForEach-Object { "$($_.Name) ($($_.Count) s)" }) -join ', '

$summary = @"

===== nijimi performance summary =====
Hardware      : $cpuName, $($gpuName.Name) @ $($gpuName.CurrentHorizontalResolution)x$($gpuName.CurrentVerticalResolution) $($gpuName.CurrentRefreshRate) Hz
Build         : $($trayProcess.ExecutablePath)
Active samples: $($active.Count) of $Seconds s with the overlay visible
Foreground    : $games

nijimi, while active
  CPU (% of one core)   : $(Get-Stats ($active | ForEach-Object AppCpuPctOfCore))
  CPU (% of total)      : $(Get-Stats ($active | ForEach-Object AppCpuPctTotal))
  GPU 3D engine (%)     : $(Get-Stats ($active | ForEach-Object AppGpu3DPct))
  GPU copy engine (%)   : $(Get-Stats ($active | ForEach-Object AppGpuCopyPct))
  GPU other engines (%) : $(Get-Stats ($active | ForEach-Object AppGpuOtherPct))
  GPU memory (MB)       : $(Get-Stats ($active | ForEach-Object AppVramMB))
  Private memory (MB)   : $(Get-Stats ($active | ForEach-Object AppPrivateMB))

Game GPU 3D engine (%)  : $(Get-Stats ($active | ForEach-Object Game3DPct))

Per-second samples: $csvPath
===========================================
"@
Write-Host $summary
if ($active.Count -eq 0) {
    Write-Warning 'The overlay was never active. Check the game has a profile and stayed in the foreground on the primary display.'
}
