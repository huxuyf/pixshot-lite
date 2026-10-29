<#
PixShot 本地 CI 流水线
======================
一条命令跑完「编译检查 → 单元/集成测试 → Debug 构建 → Release 构建 → Win7 兼容性闸门
→ 体积护栏 → GUI 端到端」，任一环节失败立即中止并返回该环节编号，便于快速定位。

设计原则
--------
- 不做任何隐式修复：只做检查和断言，不自动改写源码或产物。
- 原生命令（cargo）失败不会触发 PowerShell 异常，因此每步都显式校验 $LASTEXITCODE。
- E2E 会接管鼠标键盘约 20 秒，默认开启但可用 -SkipE2E 关闭（无桌面/CI 环境必须关闭）。
- 发布产物走 Windows 7 兼容目标（x86_64-win7-windows-gnu + -Z build-std），
  Debug 构建仍走本机默认目标以免每次迭代都重编一份 std（见 docs/win7-support-plan.md）。

用法
----
  pwsh -File scripts\ci.ps1                 # 全流程
  pwsh -File scripts\ci.ps1 -SkipE2E        # 只跑静态检查与构建
  pwsh -File scripts\ci.ps1 -MaxSizeMB 3.0  # 放宽体积护栏

参数
----
  -SkipE2E        跳过 GUI 端到端测试
  -MaxSizeMB      release 产物体积上限（默认 2.5；build-std 现场编译的 std 比预编译版大，
                  基线见 docs/win7-support-plan.md 第 5 章步骤 6）
  -TargetDir      自定义 CARGO_TARGET_DIR（默认仓库内 target\，不设置该环境变量）

退出码
------
  0  全部通过
  1  初始化/前置条件失败
  2..8  对应步骤失败（见输出中的 [n/7] 编号）
#>

[CmdletBinding()]
param(
    [switch]$SkipE2E,
    [double]$MaxSizeMB = 2.5,
    [string]$TargetDir = ''
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$root = Split-Path -Parent $PSScriptRoot
$targetRoot = if ([string]::IsNullOrWhiteSpace($TargetDir)) {
    Join-Path $root 'target'
} else {
    $TargetDir
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host '找不到 cargo，请先安装 Rust 工具链（https://rustup.rs）。' -ForegroundColor Red
    exit 1
}
if (-not (Test-Path (Join-Path $root 'Cargo.toml'))) {
    Write-Host "仓库根目录无效：$root" -ForegroundColor Red
    exit 1
}

Push-Location $root
$script:Step = 0
$script:Total = if ($SkipE2E) { 6 } else { 7 }

# 发布产物所在的目标三元组，以及发布构建所需的工具链
# （见 docs/win7-support-plan.md 第 5 章步骤 1/6）。
# 工具链刻意写在这里而不是 rust-toolchain.toml：rustup 的工具链文件**不接受**
# `<channel>-<日期>-<宿主>` 这种完整名（报 `target tuple in channel name`），
# 而只写不带宿主的 nightly 会落到 msvc 宿主、在没有 MSVC 链接器的机器上直接编不过
# （实测 GitHub runner 上的 rustup 版本会因此让 `rustc -vV` 直接失败）。
$releaseTarget = 'x86_64-win7-windows-gnu'
$releaseToolchain = 'nightly-2026-10-27-x86_64-pc-windows-gnu'

# 发布构建需要这个 nightly 与它的 rust-src（-Z build-std 要现场编译 std）。
# 缺失时直接给安装命令，别等到第 4 步才报一个难以理解的 cargo 错误。
$installedToolchains = (& rustup toolchain list 2>$null) -join "`n"
if ($installedToolchains -notmatch [regex]::Escape($releaseToolchain)) {
    Write-Host ("缺少发布构建所需的工具链 {0}，请先安装：" -f $releaseToolchain) -ForegroundColor Red
    Write-Host ("  rustup toolchain install {0} --component rust-src" -f $releaseToolchain) -ForegroundColor DarkGray
    Pop-Location
    exit 1
}

function Assert-Exit {
    param([string]$What)
    if ($null -ne $LASTEXITCODE -and $LASTEXITCODE -ne 0) {
        throw ("{0} 失败（exit {1}）" -f $What, $LASTEXITCODE)
    }
}

# 取产物的导入表文本。优先 objdump（MinGW/MSYS2，与 build.rs 查找 windres 的策略一致），
# 其次 dumpbin（MSVC）。两者都没有时**直接失败**而不是静默跳过：
# 兼容性闸门是本项目"能否在 Windows 7 启动"的唯一自动化保障，静默跳过等于没有闸门。
function Get-ImportDump {
    param([string]$Exe)
    foreach ($cand in @('objdump.exe', 'objdump')) {
        if (Get-Command $cand -ErrorAction SilentlyContinue) {
            return (& $cand -p $Exe 2>&1 | Out-String)
        }
    }
    foreach ($p in @('C:\msys64\mingw64\bin\objdump.exe', 'C:\msys64\usr\bin\objdump.exe')) {
        if (Test-Path $p) {
            return (& $p -p $Exe 2>&1 | Out-String)
        }
    }
    foreach ($cand in @('dumpbin.exe', 'dumpbin')) {
        if (Get-Command $cand -ErrorAction SilentlyContinue) {
            return (& $cand /imports $Exe 2>&1 | Out-String)
        }
    }
    throw '找不到 objdump（MSYS2/MinGW）或 dumpbin（MSVC），无法校验产物导入表'
}

function Invoke-Step {
    param(
        [string]$Name,
        [scriptblock]$Action
    )
    $script:Step++
    $n = $script:Step
    Write-Host ''
    Write-Host ('[{0}/{1}] {2}' -f $n, $script:Total, $Name) -ForegroundColor Cyan
    $sw = [Diagnostics.Stopwatch]::StartNew()
    try {
        & $Action
    } catch {
        $sw.Stop()
        Write-Host ('  x {0}' -f $_.Exception.Message) -ForegroundColor Red
        Write-Host ('  在 {0:N1}s 处中止。' -f $sw.Elapsed.TotalSeconds) -ForegroundColor DarkGray
        Pop-Location
        exit ($n + 1)
    }
    $sw.Stop()
    Write-Host ('  v 通过（{0:N1}s）' -f $sw.Elapsed.TotalSeconds) -ForegroundColor Green
}

# ---------------------------------------------------------------- 0. 环境
Write-Host ''
Write-Host 'PixShot CI' -ForegroundColor White
Write-Host ("  仓库    {0}" -f $root) -ForegroundColor DarkGray
Write-Host ("  产物    {0}" -f $targetRoot) -ForegroundColor DarkGray
Write-Host ("  E2E     {0}" -f $(if ($SkipE2E) { '跳过' } else { '执行' })) -ForegroundColor DarkGray

if (-not [string]::IsNullOrWhiteSpace($TargetDir)) {
    $env:CARGO_TARGET_DIR = $TargetDir
}

# ------------------------------------------------------- 1. 编译检查（快）
Invoke-Step 'cargo check（编译检查，含 build.rs 图标生成）' {
    cargo check --all-targets --offline
    Assert-Exit 'cargo check'
}

# ---------------------------------------------------- 2. 单元 + 集成测试
Invoke-Step 'cargo test（单元测试 + tests\ 集成测试）' {
    cargo test --offline
    Assert-Exit 'cargo test'
}

# ------------------------------------------------------------ 3. Debug 构建
Invoke-Step 'cargo build --profile dev（供 E2E 使用）' {
    cargo build --offline
    Assert-Exit 'cargo build'
}

# ---------------------------------------------------------- 4. Release 构建
# 发布产物必须走 Windows 7 兼容目标：该目标没有预编译 std，故需 -Z build-std
# （nightly 由仓库根的 rust-toolchain.toml 锁定）。默认目标编出来的产物会静态依赖
# Windows 8/10 才有的系统原语，在 Windows 7 上加载期即失败。
Invoke-Step "cargo build --release（发布产物，目标 $releaseTarget + -Z build-std）" {
    cargo +$releaseToolchain build --release --offline -Z build-std --target $releaseTarget
    Assert-Exit 'cargo build --release'
}

# ------------------------------------------------- 5. Windows 7 兼容性静态闸门
# 断言导入表里不含任何 Windows 8 / 10 才有的依赖。这是"能在 Windows 7 启动"的
# 唯一可自动化保障（真机功能验收不在 CI 能力范围内，见 docs/win7-support-plan.md 第 7 章）。
Invoke-Step 'Windows 7 兼容性闸门（产物导入表不得含 Win8+ / api-set 依赖）' {
    $exe = Join-Path $targetRoot "$releaseTarget\release\pixshot.exe"
    if (-not (Test-Path $exe)) {
        throw "未找到 $exe"
    }
    $dump = Get-ImportDump -Exe $exe
    # 黑名单两类：不存在于 NT 6.1 的 api-set / 模块，以及 DLL 在但函数不存在的入口点。
    $pattern = 'api-ms-win|bcryptprimitives|WaitOnAddress|WakeByAddress|ProcessPrng|' +
    'GetSystemTimePreciseAsFileTime|GetHostNameW|RoOriginateErrorW|' +
    'GetDpiFor|SetProcessDpiAwareness|ThreadDpiAwarenessContext|SetProcessDPIAware'
    $hit = @(($dump -split "`r?`n") | Where-Object { $_ -match $pattern })
    if ($hit.Count -gt 0) {
        $hit | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }
        throw ('产物含 {0} 处 Windows 8/10 才有的依赖，在 Windows 7 上会启动失败' -f $hit.Count)
    }
}

# ------------------------------------------------------------ 6. 体积护栏
Invoke-Step '发布体积护栏' {
    $exe = Join-Path $targetRoot "$releaseTarget\release\pixshot.exe"
    if (-not (Test-Path $exe)) {
        throw "未找到 $exe"
    }
    $mb = (Get-Item $exe).Length / 1MB
    Write-Host ('  体积 {0:N2} MB / 上限 {1:N2} MB' -f $mb, $MaxSizeMB) -ForegroundColor DarkGray
    if ($mb -gt $MaxSizeMB) {
        throw ('体积 {0:N2} MB 超出上限 {1:N2} MB —— 检查是否新增了重量级依赖或未裁剪的 crate feature' -f $mb, $MaxSizeMB)
    }
}

# -------------------------------------------------------------- 7. GUI E2E
# 用默认目标的 Debug 产物跑：E2E 验的是交互行为，不是系统兼容性
# （兼容性由第 5 步的静态闸门负责），没必要为它再编一份 win7 目标的 std。
if (-not $SkipE2E) {
    Invoke-Step 'GUI 端到端（框选 → 贴图 → 复制，会接管鼠标键盘）' {
        $e2e = Join-Path $PSScriptRoot 'gui_e2e_test.ps1'
        if (-not (Test-Path $e2e)) {
            throw "找不到 $e2e"
        }
        $debugExe = Join-Path $targetRoot 'debug\pixshot.exe'
        if (-not (Test-Path $debugExe)) {
            throw "找不到 $debugExe"
        }
        # 用独立进程执行：E2E 脚本内部以 exit 返回结果，
        # 若直接 & 调用会终止当前会话，故必须隔离。
        $hostExe = (Get-Process -Id $PID).Path
        $p = Start-Process -FilePath $hostExe `
            -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $e2e, '-Exe', $debugExe) `
            -NoNewWindow -Wait -PassThru
        if ($p.ExitCode -ne 0) {
            throw ("GUI 端到端失败（exit {0}），详见 e2e_artifacts\e2e_transcript.txt" -f $p.ExitCode)
        }
    }
}

Pop-Location

Write-Host ''
Write-Host ('全部 {0} 个步骤通过。' -f $script:Total) -ForegroundColor Green
exit 0
