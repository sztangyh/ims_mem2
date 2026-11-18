<#
.SYNOPSIS
  一键启动 ims_mem2 的服务端（ims_svc，含 MGW）与网页前端（ims_web）。

.DESCRIPTION
  在 ims_mem2 目录下运行（脚本会自定位到 ims_mem2，可双击 / 任意 CWD 调用）。
  启动后自动等待端口就绪，打印访问地址与日志路径。

  默认（可改参）：
    端口 : ims_svc=127.0.0.1:7711 · ims_web=127.0.0.1:7788
    数据 : 上一级 \agcf_data\ALLME_2026*.txt（AGCF）+ C:\dev\ims_ws\data\mgw（MGW）
    日志 : .temp\logs\svc-<时间>.log / web-<时间>.log（导入/监听信息在 err 日志）

.PARAMETER SvcPort    ims_svc 监听端口（默认 7711）
.PARAMETER WebPort    ims_web 监听端口（默认 7788）
.PARAMETER AgfcData   AGCF 数据（默认 上一级 \agcf_data\ALLME_2026*.txt；* 由 ims_svc 自行展开）
.PARAMETER MgwData    MGW 数据目录（默认 C:\dev\ims_ws\data\mgw；-NoMgw 时不加载）
.PARAMETER NoMgw      不加载 MGW
.PARAMETER Stop        只停止本机所有 ims_svc / ims_web 进程后退出
.PARAMETER Restart     先停旧进程，再重新启动

.EXAMPLE
  .\start-dev.ps1                    # 默认全量启动（AGCF+MGW，7711/7788）
  .\start-dev.ps1 -WebPort 8080      # 改 web 端口
  .\start-dev.ps1 -NoMgw             # 不带 MGW
  .\start-dev.ps1 -Restart           # 重启
  .\start-dev.ps1 -Stop              # 停止
#>
[CmdletBinding()]
param(
  [int]    $SvcPort  = 7711,
  [int]    $WebPort  = 7788,
  [string] $AgfcData = '',
  [string] $MgwData  = 'C:\dev\ims_ws\data\mgw',
  [switch] $NoMgw,
  [switch] $Stop,
  [switch] $Restart
)

$ErrorActionPreference = 'Stop'
$root   = $PSScriptRoot
$svcExe = Join-Path $root 'target\release\ims_svc.exe'
$webExe = Join-Path $root 'target\release\ims_web.exe'
if (-not $AgfcData) { $AgfcData = Join-Path (Split-Path $root) 'agcf_data\ALLME_2026*.txt' }
$logDir = Join-Path $root '.temp\logs'
New-Item -ItemType Directory -Path $logDir -Force | Out-Null
# 空 stdin 文件：把子进程的 stdin 指到这里，让常驻服务完全脱离父 PowerShell 的句柄
# （否则父进程会一直等它的 stdin 句柄关闭，脚本/调用方会“吊死”）。
$nullIn = Join-Path $logDir 'stdin.nul'
if (-not (Test-Path $nullIn)) { '' | Set-Content -LiteralPath $nullIn -NoNewline }

function Test-Port([int]$port) {
  $c = New-Object System.Net.Sockets.TcpClient
  try { $c.Connect('127.0.0.1', $port); return $true } catch { return $false } finally { $c.Dispose() }
}
function Wait-Port([int]$Port, [int]$TimeoutSec = 60) {
  $deadline = (Get-Date).AddSeconds($TimeoutSec)
  while ((Get-Date) -lt $deadline) { if (Test-Port $Port) { return $true }; Start-Sleep -Milliseconds 800 }
  return $false
}
function Get-ImsProc {
  Get-CimInstance Win32_Process -Filter "Name='ims_svc.exe' OR Name='ims_web.exe'" -ErrorAction SilentlyContinue
}

# ---------- 停止 / 重启 ----------
if ($Stop -or $Restart) {
  $p = Get-ImsProc
  if ($p) {
    $names = ($p.ProcessId | ForEach-Object { (Get-Process -Id $_ -ErrorAction SilentlyContinue).ProcessName } | Sort-Object -Unique) -join ', '
    Write-Host "停止 $($p.Count) 个进程（$names）…"
    foreach ($proc in $p) { taskkill.exe /PID $proc.ProcessId /T /F | Out-Null }
    Start-Sleep -Seconds 1
  } else { Write-Host '没有找到 ims_svc / ims_web 进程。' }
  if ($Stop) { exit 0 }
}

# ---------- 前置检查 ----------
foreach ($exe in @($svcExe, $webExe)) {
  if (-not (Test-Path $exe)) { Write-Error "未找到 $exe；请先 cargo build --release -p ims_svc -p ims_web"; exit 2 }
}
if (-not (Test-Path (Join-Path $root 'mgw_ne_map.tsv'))) { Write-Warning '未找到 mgw_ne_map.tsv，MGW 网元映射可能缺失。' }

# ---------- 端口占用判定 ----------
$needSvc = -not (Test-Port $SvcPort)
$needWeb = -not (Test-Port $WebPort)
if (-not $needSvc) { Write-Warning "端口 $SvcPort 已被占用，跳过 ims_svc（用 -Restart 可先重启）。" }
if (-not $needWeb) { Write-Warning "端口 $WebPort 已被占用，跳过 ims_web（用 -Restart 可先重启）。" }
if (-not ($needSvc -or $needWeb)) { Write-Host '两个端口都已在监听，无需启动。'; exit 0 }

# ---------- 启动 ims_svc ----------
if ($needSvc) {
  $ts = Get-Date -Format 'yyyyMMdd-HHmmss'
  $svcOut = Join-Path $logDir "svc-$ts.log"
  $svcErr = Join-Path $logDir "svc-$ts.err.log"
  $a = @("127.0.0.1:$SvcPort", $AgfcData)
  if (-not $NoMgw) { $a += '--mgw', $MgwData }
  Write-Host "启动 ims_svc → 127.0.0.1:$SvcPort  （$($a -join ' ')）"
  Start-Process -FilePath $svcExe -ArgumentList $a -WorkingDirectory $root -WindowStyle Hidden `
    -RedirectStandardOutput $svcOut -RedirectStandardError $svcErr -RedirectStandardInput $nullIn
  if (-not (Wait-Port $SvcPort 90)) { Write-Error "ims_svc 未在 $SvcPort 就绪，详见 $svcErr"; exit 3 }
  Write-Host "  ims_svc 就绪（$SvcPort）。日志: $svcErr"
}

# ---------- 启动 ims_web ----------
if ($needWeb) {
  $ts = Get-Date -Format 'yyyyMMdd-HHmmss'
  $webOut = Join-Path $logDir "web-$ts.log"
  $webErr = Join-Path $logDir "web-$ts.err.log"
  $a = @('--svc', "127.0.0.1:$SvcPort", '--http', "127.0.0.1:$WebPort")
  Write-Host "启动 ims_web → http://127.0.0.1:$WebPort"
  Start-Process -FilePath $webExe -ArgumentList $a -WorkingDirectory $root -WindowStyle Hidden `
    -RedirectStandardOutput $webOut -RedirectStandardError $webErr -RedirectStandardInput $nullIn
  if (-not (Wait-Port $WebPort 15)) { Write-Error "ims_web 未在 $WebPort 就绪，详见 $webErr"; exit 4 }
  Write-Host "  ims_web 就绪（$WebPort）。日志: $webErr"
}

# ---------- 汇总 ----------
Write-Host ''
Write-Host '═══════ 已启动 ═══════'
Write-Host "  浏览器访问 : http://127.0.0.1:$WebPort"
Write-Host "  ims_svc    : 127.0.0.1:$SvcPort"
Write-Host '  停止        : .\start-dev.ps1 -Stop'
Write-Host "  日志目录   : $logDir"