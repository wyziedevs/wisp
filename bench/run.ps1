<#
Wisp vs ASP.NET Core on this machine.

Builds both apps in release mode, then runs each server alone on the first
half of the logical CPUs and the load generator on the other half, so the
two never compete for a core. Prints throughput, latency, response size,
memory and time to first response.

  powershell -ExecutionPolicy Bypass -File bench\run.ps1 [-Connections 64] [-Duration 10] [-Warmup 5]
#>
param([int]$Connections = 64, [int]$Duration = 10, [int]$Warmup = 5)
$ErrorActionPreference = 'Stop'

$repo = Split-Path $PSScriptRoot -Parent
$cpus = [Environment]::ProcessorCount
$half = [int]($cpus / 2)
$self = [Diagnostics.Process]::GetCurrentProcess()
$allMask = $self.ProcessorAffinity
$serverMask = [IntPtr]((1L -shl $half) - 1)
$loadMask = [IntPtr](((1L -shl $cpus) - 1) -bxor ((1L -shl $half) - 1))

Push-Location $repo
try {
    cargo build --release -q -p wisp-bench -p wisp-load
    if ($LASTEXITCODE) { throw 'cargo build failed' }
    dotnet publish "$PSScriptRoot\aspnet" -c Release -o "$PSScriptRoot\aspnet\out" --nologo -v q
    if ($LASTEXITCODE) { throw 'dotnet publish failed' }
} finally { Pop-Location }
$load = "$repo\target\release\wisp-load.exe"

# Child processes inherit the parent's affinity mask, so set ours around
# each launch. This pins the process from its first instruction, before a
# runtime sizes its thread pool.
function Start-Pinned([string]$exe, [hashtable]$vars) {
    foreach ($k in $vars.Keys) { Set-Item "env:$k" $vars[$k] }
    $self.ProcessorAffinity = $serverMask
    try { return Start-Process $exe -WorkingDirectory (Split-Path $exe) -NoNewWindow -PassThru }
    finally { $self.ProcessorAffinity = $allMask }
}

# Milliseconds from launch until the first successful response.
function Wait-Ready($proc, [int]$port) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($true) {
        if ($proc.HasExited) { throw "server exited with code $($proc.ExitCode)" }
        if ($sw.ElapsedMilliseconds -gt 30000) { throw "server did not start within 30s" }
        # A short connect timeout: refused connects take 2s to fail on Windows.
        $c = New-Object Net.Sockets.TcpClient
        $open = $c.ConnectAsync('127.0.0.1', $port).Wait(25)
        $c.Dispose()
        if ($open) {
            try {
                $null = (New-Object Net.WebClient).DownloadString("http://127.0.0.1:$port/plaintext")
                return $sw.ElapsedMilliseconds
            } catch { }
        }
        Start-Sleep -Milliseconds 5
    }
}

$results = @()
function Measure-Server([string]$name, [string]$exe, [int]$port, [hashtable]$vars, [string[]]$paths) {
    $proc = Start-Pinned $exe $vars
    try {
        $ready = Wait-Ready $proc $port
        Write-Host "`n== $name  (first response after $ready ms)" -ForegroundColor Cyan
        foreach ($path in $paths) {
            $proc.Refresh()
            $cpu0 = $proc.TotalProcessorTime
            $wall = [Diagnostics.Stopwatch]::StartNew()
            $self.ProcessorAffinity = $loadMask
            try { $out = & $load "http://127.0.0.1:$port$path" -c $Connections -d $Duration -w $Warmup }
            finally { $self.ProcessorAffinity = $allMask }
            $proc.Refresh()
            # Average cores the server kept busy. Throughput can be capped by
            # the load generator or the OS network stack; CPU per request is
            # what the server itself costs.
            $cores = ($proc.TotalProcessorTime - $cpu0).TotalSeconds / $wall.Elapsed.TotalSeconds
            $out | ForEach-Object { Write-Host $_ }
            $text = $out -join "`n"
            $rps = [double]([regex]::Match($text, '([\d.]+) req/s').Groups[1].Value)
            Write-Host ("  server    {0:N1} of $half cores busy, {1:N1} CPU us per request" -f $cores, ($cores * 1e6 / $rps))
            $script:results += [pscustomobject]@{
                Server        = $name
                Path          = $path
                'req/s'       = [int]$rps
                p50           = [regex]::Match($text, 'p50 (\S+)').Groups[1].Value
                p99           = [regex]::Match($text, 'p99 (\S+)').Groups[1].Value
                'p99.9'       = [regex]::Match($text, 'p99\.9 (\S+)').Groups[1].Value
                'CPU us/req'  = [math]::Round($cores * 1e6 / $rps, 1)
                Bytes         = [int]([regex]::Match($text, '(\d+) per response').Groups[1].Value)
                'Peak MB'     = [math]::Round($proc.PeakWorkingSet64 / 1MB)
                'Start ms'    = $ready
            }
        }
    } finally {
        Stop-Process $proc -Force
        $proc.WaitForExit()
    }
}

Write-Host "servers on $half logical CPUs, load generator on $($cpus - $half); $Connections connections, ${Warmup}s warmup, ${Duration}s measured"

Measure-Server 'Wisp' "$repo\target\release\wisp-bench.exe" 3401 `
    @{ HOST = '127.0.0.1'; PORT = '3401'; WISP_THREADS = "$half" } `
    @('/plaintext', '/fortunes')

Measure-Server 'ASP.NET Core' "$PSScriptRoot\aspnet\out\Bench.exe" 3402 `
    @{ ASPNETCORE_URLS = 'http://127.0.0.1:3402' } `
    @('/plaintext', '/fortunes', '/fortunes-blazor')

$results | Format-Table -AutoSize | Out-String | Write-Host
