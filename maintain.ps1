#Requires -Version 5.1
# ============================================================
#  maintain.ps1 — DSH Control Panel 项目维护工具（WinForms GUI）
#
#  兼容：Windows PowerShell 5.1 与 PowerShell 7 (pwsh, Windows)
#  - 文件保存为 UTF-8 with BOM（5.1 中文解析必需，7 同样兼容）
#  - 仅使用两者共有的语法：不用 ?? / ?: / && / ForEach-Object -Parallel
#    / -Encoding utf8NoBOM / Get-Content -AsByteStream / ArgumentList 等
#  - 由 maintain.bat 以 -Sta -WindowStyle Hidden 拉起
#
#  用法：maintain.bat           （启动 GUI）
#        maintain.ps1 -SelfTest （仅做语法/环境自检，不启动界面）
# ============================================================
param([switch]$SelfTest)

if ($SelfTest) {
    Write-Host "maintain.ps1 语法/加载自检通过 (PowerShell $($PSVersionTable.PSVersion))"
    exit 0
}

$ErrorActionPreference = 'Stop'

# ---------- WinForms 加载 ----------
try {
    Add-Type -AssemblyName System.Windows.Forms
    Add-Type -AssemblyName System.Drawing
} catch {
    Write-Error "无法加载 WinForms（需在 Windows 交互桌面运行）：$($_.Exception.Message)"
    exit 1
}

# ---------- 常量 ----------
$script:RepoUrl     = 'https://github.com/CrandyChen/dsh-control-panel.git'
$script:RepoWeb     = 'https://github.com/CrandyChen/dsh-control-panel'
$script:RepoActions = 'https://github.com/CrandyChen/dsh-control-panel/actions'
$script:RepoReleases= 'https://github.com/CrandyChen/dsh-control-panel/releases'
$script:RepoRoot    = $PSScriptRoot
if (-not $script:RepoRoot) { $script:RepoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path }

# ---------- 全局状态 ----------
$script:LogBox        = $null
$script:MainForm      = $null
$script:CurrentProc   = $null
$script:StopRequested = $false
$script:ActionButtons = @()
$script:BtnStop       = $null
$script:BtnRefresh    = $null
$script:EvtSeq        = 0
$script:LogTimer      = $null
$script:LogQueue      = $null
$script:LblDir = $script:LblRepo = $script:LblBranch = $script:LblVersion = $null

# ============================================================
#  基础工具
# ============================================================

# 日志采用「任意线程入队 + UI 线程 Timer 定时刷新」：跨线程/后台线程只入队，
# 由 Timer(Tick) 在 UI 线程写 RichTextBox，并按级别着色。
function Append-Log {
    param([string]$Text, [string]$Level = 'INFO')
    $line = switch ($Level) {
        'WARN'  { "[WARN] $Text" }
        'ERROR' { "[ERROR] $Text" }
        default { $Text }
    }
    # 携带级别入队，供 UI 线程按级别着色
    if ($script:LogQueue) { $script:LogQueue.Enqueue(@{ Text = $line; Kind = $Level }) }
}

# 结果报告：根据退出码给出明确的成功 / 失败结论（成功→绿，失败→红）。
function Show-Result {
    param([string]$Action, [int]$Code)
    if ($Code -eq 0) {
        Append-Log ("✅ {0} 成功" -f $Action) 'OK'
    } else {
        Append-Log ("❌ {0} 失败（退出码 {1}）" -f $Action, $Code) 'ERROR'
    }
}

function Show-Message {
    param([string]$Title, [string]$Text, [string]$Kind = 'Info')
    $icon = switch ($Kind) {
        'Warning'  { [System.Windows.Forms.MessageBoxIcon]::Warning }
        'Error'    { [System.Windows.Forms.MessageBoxIcon]::Error }
        'Question' { [System.Windows.Forms.MessageBoxIcon]::Question }
        default    { [System.Windows.Forms.MessageBoxIcon]::Information }
    }
    if ($script:MainForm -and -not $script:MainForm.IsDisposed) {
        $null = [System.Windows.Forms.MessageBox]::Show($script:MainForm, $Text, $Title, [System.Windows.Forms.MessageBoxButtons]::OK, $icon)
    } else {
        $null = [System.Windows.Forms.MessageBox]::Show($Text, $Title, [System.Windows.Forms.MessageBoxButtons]::OK, $icon)
    }
}

function Confirm-Message {
    param([string]$Title, [string]$Text)
    if ($script:MainForm -and -not $script:MainForm.IsDisposed) {
        $r = [System.Windows.Forms.MessageBox]::Show($script:MainForm, $Text, $Title,
            [System.Windows.Forms.MessageBoxButtons]::YesNo, [System.Windows.Forms.MessageBoxIcon]::Question)
    } else {
        $r = [System.Windows.Forms.MessageBox]::Show($Text, $Title,
            [System.Windows.Forms.MessageBoxButtons]::YesNo, [System.Windows.Forms.MessageBoxIcon]::Question)
    }
    return ($r -eq [System.Windows.Forms.DialogResult]::Yes)
}

function Set-Busy {
    param([bool]$Busy)
    foreach ($b in $script:ActionButtons) {
        if ($b) { $b.Enabled = -not $Busy }
    }
    if ($script:BtnStop)    { $script:BtnStop.Enabled    = $Busy }
    if ($script:BtnRefresh) { $script:BtnRefresh.Enabled = -not $Busy }
}

# 统一解析 git 可执行文件（兼容 git.exe 与 PATH 上的 git）。
function Get-GitExecutable {
    $cmd = Get-Command git.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $cmd = Get-Command git -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return $null
}

function Get-RepoValid {
    return ((Test-Path (Join-Path $script:RepoRoot '.git')) -and
            (Test-Path (Join-Path $script:RepoRoot 'package.json')))
}

function Assert-Repo {
    if (-not (Get-RepoValid)) {
        Show-Message '不在项目目录' ("当前目录不是有效的项目仓库：`n$script:RepoRoot`n请在项目根目录运行 maintain.bat。") 'Warning'
        return $false
    }
    return $true
}

# 判断工作区是否有未提交改动（含新增/修改/删除文件）。
function Test-WorktreeDirty {
    return ((Get-GitStatusFiles).Count -gt 0)
}

# 分支切换类操作前的工作区确认：无改动直接通过；有改动则弹出确认（改动会在无冲突时被带到目标分支）。
function Confirm-CleanOrContinue {
    param([string]$ActionDesc)
    if (-not (Test-WorktreeDirty)) { return $true }
    return (Confirm-Message '工作区有未提交改动' ("检测到未提交的改动。`n$ActionDesc 会先切换分支，未提交改动会在无冲突时被一并带到目标分支。`n`n确认继续？"))
}

function Get-CurrentBranch {
    try {
        $git = Get-GitExecutable
        if (-not $git) { return '—' }
        $b = (& $git -C $script:RepoRoot rev-parse --abbrev-ref HEAD 2>$null | Select-Object -First 1)
        if ($b) { return $b.Trim() }
        return '—'
    } catch { return '—' }
}

function Get-CurrentVersion {
    try {
        $p = Join-Path $script:RepoRoot 'package.json'
        if (-not (Test-Path $p)) { return '—' }
        $c = [System.IO.File]::ReadAllText($p)
        return (($c | ConvertFrom-Json).version)
    } catch { return '—' }
}

function Refresh-Status {
    if ($script:LblDir)     { $script:LblDir.Text = "目录: $($script:RepoRoot)" }
    if ($script:LblRepo) {
        $valid = Get-RepoValid
        $script:LblRepo.Text = if ($valid) { '仓库: 有效' } else { '仓库: 无效（需在项目根目录运行）' }
        $script:LblRepo.ForeColor = if ($valid) { [System.Drawing.Color]::SeaGreen } else { [System.Drawing.Color]::Firebrick }
    }
    if ($script:LblBranch)  { $script:LblBranch.Text = "分支: $(Get-CurrentBranch)" }
    if ($script:LblVersion) { $script:LblVersion.Text = "版本: $(Get-CurrentVersion)" }
}

# ============================================================
#  进程执行（异步流式输出，兼容 5.1 / 7）
# ============================================================

function Quote-Arg {
    param([string]$Arg)
    if ([string]::IsNullOrEmpty($Arg)) { return '""' }
    if ($Arg -match '^[A-Za-z0-9_\-./:=@+%]+$') { return $Arg }
    # Windows 命令行参数转义：双引号前加反斜杠，字符串尾部反斜杠加倍
    $esc = $Arg -replace '(\\+)"', '$1$1\"' -replace '"', '\"'
    if ($esc -match '(\\+)$') { $esc = $esc -replace '(\\+)$', '$1$1' }
    return ('"' + $esc + '"')
}

function Invoke-Process {
    param(
        [string]$FileName,
        [string[]]$Arguments,
        [string]$WorkingDirectory = $script:RepoRoot,
        [hashtable]$Environment = @{},
        [string]$StderrLevel = 'WARN',
        # 日志中展示的命令字符串（如 "git pull --ff-only"）；缺省时自动从 FileName/Arguments 推导
        [string]$DisplayCommand = $null
    )
    $argLine = ($Arguments | ForEach-Object { Quote-Arg $_ }) -join ' '
    # 每条命令执行前写入日志（便于审计操作）。pnpm 经 cmd.exe /d /s /c 包装，展示真实命令。
    if (-not $DisplayCommand) {
        if ($FileName -match 'cmd\.exe$' -and $Arguments.Count -ge 4) {
            $DisplayCommand = (($Arguments | Select-Object -Skip 3) -join ' ')
        } else {
            $DisplayCommand = "$FileName $argLine"
        }
    }
    Append-Log "▶ $DisplayCommand" 'CMD'
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $FileName
    $psi.Arguments = $argLine
    $psi.WorkingDirectory = $WorkingDirectory
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true
    try { $psi.StandardOutputEncoding = [System.Text.Encoding]::UTF8 } catch { }
    try { $psi.StandardErrorEncoding = [System.Text.Encoding]::UTF8 } catch { }
    foreach ($k in $Environment.Keys) {
        try { $psi.EnvironmentVariables[$k] = [string]$Environment[$k] } catch { }
    }

    $proc = New-Object System.Diagnostics.Process
    $proc.StartInfo = $psi
    try {
        $null = $proc.Start()
    } catch {
        $script:CurrentProc = $null
        if ($proc) { $proc.Dispose() }
        throw
    }
    $script:CurrentProc = $proc

    # 输出流式显示：用 Register-ObjectEvent 订阅 DataReceived 事件。
    # 事件 Action 在 runspace 空闲（Start-Sleep 让出）时于 UI 线程执行并写入队列，
    # 由日志 Timer 统一刷新 —— 全程无跨线程更新控件。
    $script:EvtSeq += 1
    $idOut = "dsh-out-$($script:EvtSeq)"
    $idErr = "dsh-err-$($script:EvtSeq)"
    $script:CurrentStderrLevel = $StderrLevel
    try {
        $null = Register-ObjectEvent -InputObject $proc -EventName OutputDataReceived -SourceIdentifier $idOut -Action {
            if ($EventArgs -and $EventArgs.Data) { Append-Log $EventArgs.Data }
        }
        $null = Register-ObjectEvent -InputObject $proc -EventName ErrorDataReceived -SourceIdentifier $idErr -Action {
            if ($EventArgs -and $EventArgs.Data) {
                # git 等命令把大量正常提示写到 stderr：Info 级时仅把明显的错误行标红
                $lvl = $script:CurrentStderrLevel
                if ($lvl -eq 'INFO' -and ($EventArgs.Data -match '(?i)(fatal:|^fatal|^error|error:|failed|conflict|rejected)')) {
                    $lvl = 'ERROR'
                }
                Append-Log $EventArgs.Data $lvl
            }
        }
        $proc.BeginOutputReadLine()
        $proc.BeginErrorReadLine()

        $stopped = $false
        while (-not $proc.HasExited) {
            [System.Windows.Forms.Application]::DoEvents()
            Start-Sleep -Milliseconds 30
            if ($script:StopRequested) {
                $script:StopRequested = $false
                $stopped = $true
                Append-Log '用户请求停止，正在终止任务…' 'WARN'
                Append-Log "▶ taskkill.exe /PID $($proc.Id) /T /F" 'CMD'
                & "$env:SystemRoot\System32\taskkill.exe" /PID $proc.Id /T /F 2>$null | Out-Null
                break
            }
        }
        try { $proc.WaitForExit() } catch { }
        $code = if ($stopped) { -1 } else { $proc.ExitCode }
        return $code
    } finally {
        # 无论是否异常都清理：注销事件、移除残留事件、释放进程、清空 CurrentProc
        try { Unregister-Event -SourceIdentifier $idOut -ErrorAction SilentlyContinue } catch { }
        try { Unregister-Event -SourceIdentifier $idErr -ErrorAction SilentlyContinue } catch { }
        Get-Event -ErrorAction SilentlyContinue |
            Where-Object { $_.SourceIdentifier -eq $idOut -or $_.SourceIdentifier -eq $idErr } |
            Remove-Event -ErrorAction SilentlyContinue
        if ($proc) { $proc.Dispose() }
        $script:CurrentProc = $null
    }
}

function Invoke-Git {
    param([string[]]$Arguments, [string]$WorkingDirectory = $script:RepoRoot)
    $git = Get-GitExecutable
    if (-not $git) {
        Append-Log '未找到 git，请先安装（见「环境依赖自检」指引）' 'ERROR'
        return -1
    }
    # GIT_TERMINAL_PROMPT=0：避免 git 在凭据提示时挂起等待输入
    # git 大量正常提示写在 stderr（如 To https://…、Already up to date.），以 INFO（灰色）流出，
    # 真正的 error/fatal 行会在上方被标为 ERROR（红色）。
    return Invoke-Process -FileName $git -Arguments $Arguments -WorkingDirectory $WorkingDirectory `
        -Environment @{ 'GIT_TERMINAL_PROMPT' = '0' } -StderrLevel 'INFO' `
        -DisplayCommand ("git " + (($Arguments | ForEach-Object { Quote-Arg $_ }) -join ' '))
}

function Invoke-Pnpm {
    param([string[]]$Arguments, [string]$WorkingDirectory = $script:RepoRoot)
    # pnpm 是 .cmd，需经 cmd.exe 执行
    $cmdline = 'pnpm ' + (($Arguments | ForEach-Object { Quote-Arg $_ }) -join ' ')
    # 无控制台（maintain.ps1 由 maintain.bat 以 Hidden 拉起）下 pnpm 无法交互，
    # CI=true 与 confirmModulesPurge=false 使其自动清空/重装 node_modules 而不弹确认。
    return Invoke-Process -FileName "$env:SystemRoot\System32\cmd.exe" `
        -Arguments @('/d', '/s', '/c', $cmdline) -WorkingDirectory $WorkingDirectory `
        -Environment @{ 'CI' = 'true'; 'npm_config_confirm_modules_purge' = 'false' } `
        -DisplayCommand ("pnpm " + (($Arguments | ForEach-Object { Quote-Arg $_ }) -join ' '))
}

# ============================================================
#  环境依赖检查
# ============================================================

function New-TestResult {
    param([string]$Id, [bool]$Ok, [string]$Version, [string]$Guide)
    return @{ Id = $Id; Ok = $Ok; Version = $Version; Guide = $Guide }
}

function Test-Node {
    $cmd = Get-Command node -ErrorAction SilentlyContinue
    if (-not $cmd) {
        return New-TestResult 'Node.js' $false $null (
            "未检测到 Node.js。`n安装指引：`n  1) 打开 https://nodejs.org 下载 LTS（要求 ≥ 22.19 或 ≥ 24）`n  2) 安装时勾选 Add to PATH`n  3) 完成后重新运行本工具")
    }
    $v = (& node --version 2>$null | Select-Object -First 1)
    $major = 0
    if ($v -match 'v?(\d+)') { $major = [int]$Matches[1] }
    if ($major -ge 22) { return New-TestResult 'Node.js' $true $v $null }
    return New-TestResult 'Node.js' $false $v (
        "Node.js 版本过低（$v），建议 ≥ 22.19。`n请到 https://nodejs.org 下载新版后重装。")
}

function Test-Pnpm {
    $cmd = Get-Command pnpm -ErrorAction SilentlyContinue
    if (-not $cmd) {
        return New-TestResult 'pnpm' $false $null (
            "未检测到 pnpm。`n安装指引：`n  1) 安装 Node.js 后执行：npm install -g pnpm`n     或：corepack enable && corepack prepare pnpm@latest --activate`n  2) 验证：pnpm --version（建议 ≥ 11.7）")
    }
    $v = (& pnpm --version 2>$null | Select-Object -First 1)
    return New-TestResult 'pnpm' $true $v $null
}

function Test-Rust {
    $cmd = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $cmd) {
        return New-TestResult 'Rust (rustup/cargo)' $false $null (
            "未检测到 Rust。`n安装指引：`n  1) 打开 https://rustup.rs 下载 rustup-init.exe`n  2) 运行并选择默认安装（stable toolchain + MSVC 目标）`n  3) 完成后重开终端验证：cargo --version")
    }
    $v = (& cargo --version 2>$null | Select-Object -First 1)
    return New-TestResult 'Rust (rustup/cargo)' $true $v $null
}

function Test-VsCpp {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (Test-Path $vswhere) {
        $path = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
        if ($path) { return New-TestResult 'VS C++ Build Tools' $true ($path.Trim()) $null }
    }
    return New-TestResult 'VS C++ Build Tools' $false $null (
        "未检测到 Visual Studio C++ Build Tools。`n安装指引：`n  1) 打开 https://visualstudio.microsoft.com/visual-cpp-build-tools/ 下载 Build Tools`n  2) 运行安装器，勾选工作负载「使用 C++ 的桌面开发」(Desktop development with C++)`n  3) 组件保持默认（含 MSVC v143 + Windows SDK），等待安装完成`n  4) 完成后重新运行本工具`n`n说明：Rust 后端通过 git2 内置 libgit2（vendored 编译 C 库），需要 MSVC C++ 工具链。")
}

function Test-Git {
    $git = Get-GitExecutable
    if (-not $git) {
        return New-TestResult 'Git' $false $null (
            "未检测到 Git。`n安装指引：`n  1) 打开 https://git-scm.com/download/win 下载安装（默认选项即可）`n  2) 完成后重开本工具")
    }
    $v = (& $git --version 2>$null | Select-Object -First 1)
    return New-TestResult 'Git' $true $v $null
}

function Assert-Env {
    param([string[]]$Ids)
    $results = @()
    foreach ($id in $Ids) {
        $r = switch ($id) {
            'node'  { Test-Node }
            'pnpm'  { Test-Pnpm }
            'rust'  { Test-Rust }
            'vscpp' { Test-VsCpp }
            'git'   { Test-Git }
        }
        $results += $r
    }
    $missing = $results | Where-Object { -not $_.Ok }
    if ($missing) {
        # 有缺失项时逐项列出，并弹窗指引
        foreach ($r in $results) {
            Append-Log ("{0}  {1}  {2}" -f $(if ($r.Ok) { 'OK  ' } else { 'MISS' }), $r.Id, $r.Version)
        }
        $msg = ($missing | ForEach-Object { "• $($_.Id)`n$($_.Guide)" }) -join "`n`n"
        Show-Message '缺少必要环境' $msg 'Warning'
        return $false
    }
    # 全部通过时只打一行总览，避免每次操作前都刷 5 行
    Append-Log ("✔ 环境检查通过：{0}" -f (($results | ForEach-Object { $_.Id }) -join ' / ')) 'OK'
    return $true
}

# ============================================================
#  业务逻辑
# ============================================================

function New-BranchName {
    param([string]$Type, [string]$RawName)
    $s = $RawName.Trim()
    if (-not $s) { return $null }
    $s = $s.ToLowerInvariant()
    $s = [regex]::Replace($s, '[^a-z0-9]+', '-')
    $s = $s.Trim('-')
    if (-not $s) { return $null }
    return ($Type + '/' + $s)
}

function Set-VersionAll {
    param([string]$NewVersion, [switch]$DryRun)
    $enc = New-Object System.Text.UTF8Encoding($false)
    # 用捕获组保留原始缩进/分隔符，仅替换版本号数字，容忍空格差异（更稳健）
    $pairs = @(
        @{ Path = 'package.json';            Pattern = '("version"\s*:\s*")\d+\.\d+\.\d+(")';             Replacement = ('${1}' + $NewVersion + '${2}') },
        @{ Path = 'src-tauri\tauri.conf.json'; Pattern = '("version"\s*:\s*")\d+\.\d+\.\d+(")';             Replacement = ('${1}' + $NewVersion + '${2}') },
        @{ Path = 'src-tauri\Cargo.toml';    Pattern = '(?m)^(version\s*=\s*")\d+\.\d+\.\d+(")';             Replacement = ('${1}' + $NewVersion + '${2}') },
        @{ Path = 'src\constants.ts';        Pattern = '(APP_VERSION\s*=\s*")\d+\.\d+\.\d+(")';             Replacement = ('${1}' + $NewVersion + '${2}') }
    )
    # 第一遍：读取并校验所有文件都能匹配，任一失败则中止（不写入任何文件）。
    $contents = @()
    foreach ($pair in $pairs) {
        $full = Join-Path $script:RepoRoot $pair.Path
        if (-not (Test-Path $full)) { throw "文件不存在：$($pair.Path)" }
        $c = [System.IO.File]::ReadAllText($full)
        if ($c -notmatch $pair.Pattern) { throw "未找到可替换的版本号：$($pair.Path)" }
        $contents += [pscustomobject]@{ Path = $pair.Path; Full = $full; Pattern = $pair.Pattern; Replacement = $pair.Replacement; Text = $c }
    }
    # Cargo.lock：只改根包（多行锚定，避免误改依赖版本）
    $lock = Join-Path $script:RepoRoot 'src-tauri\Cargo.lock'
    if (-not (Test-Path $lock)) { throw '文件不存在：src-tauri\Cargo.lock' }
    $l = [System.IO.File]::ReadAllText($lock)
    $lockPat = '(\[\[package\]\]\r?\nname = "dsh-control-panel"\r?\nversion = ")\d+\.\d+\.\d+(")'
    if ($l -notmatch $lockPat) { throw 'Cargo.lock 未找到根包 dsh-control-panel 的版本号' }

    if ($DryRun) {
        Append-Log ('[试运行] 将把版本号更新为 ' + $NewVersion + '（共 5 个文件）')
        return
    }

    # 第二遍：全部校验通过后统一写入。
    foreach ($it in $contents) {
        $c = [regex]::Replace($it.Text, $it.Pattern, $it.Replacement)
        [System.IO.File]::WriteAllText($it.Full, $c, $enc)
        Append-Log ("已更新 {0}  ->  {1}" -f $it.Path, $NewVersion)
    }
    $l2 = [regex]::Replace($l, $lockPat, ('${1}' + $NewVersion + '${2}'))
    [System.IO.File]::WriteAllText($lock, $l2, $enc)
    Append-Log ("已更新 {0}  ->  {1}" -f 'src-tauri\Cargo.lock', $NewVersion)
}

function Open-Url {
    param([string]$Url)
    try { Start-Process $Url } catch { Append-Log ("打开链接失败：$Url") 'WARN' }
}

# ============================================================
#  输入表单（模态）
# ============================================================

function New-FormDialog {
    param([string]$Title, [int]$Width = 420, [int]$Height = 240)
    $f = New-Object System.Windows.Forms.Form
    $f.Text = $Title
    $f.Size = New-Object System.Drawing.Size($Width, $Height)
    $f.StartPosition = 'CenterParent'
    $f.FormBorderStyle = 'FixedDialog'
    $f.MaximizeBox = $false
    $f.MinimizeBox = $false
    return $f
}

function New-DialogButtons {
    param([System.Windows.Forms.Form]$Form, [int]$Y, [string]$OkText = '确定')
    $btnOk = New-Object System.Windows.Forms.Button
    $btnOk.Text = $OkText
    $btnOk.Size = New-Object System.Drawing.Size(90, 30)
    # 注意：构造函数参数列表里逗号优先级高于减法（$w - 200, $y 会被解析成数组减法），必须加括号
    $btnOk.Location = New-Object System.Drawing.Point(($Form.ClientSize.Width - 200), $Y)
    $btnOk.DialogResult = 'OK'
    $btnCancel = New-Object System.Windows.Forms.Button
    $btnCancel.Text = '取消'
    $btnCancel.Size = New-Object System.Drawing.Size(90, 30)
    $btnCancel.Location = New-Object System.Drawing.Point(($Form.ClientSize.Width - 104), $Y)
    $btnCancel.DialogResult = 'Cancel'
    $Form.Controls.AddRange(@($btnOk, $btnCancel))
    $Form.AcceptButton = $btnOk
    $Form.CancelButton = $btnCancel
}

function Show-GitConfigForm {
    $f = New-FormDialog '设置全局 Git 配置' 460 240
    $lblName = New-Object System.Windows.Forms.Label
    $lblName.Text = 'user.name（如 CrandyChen）'
    $lblName.Location = New-Object System.Drawing.Point(16, 14)
    $lblName.AutoSize = $true
    $txtName = New-Object System.Windows.Forms.TextBox
    $txtName.Location = New-Object System.Drawing.Point(16, 40)
    $txtName.Size = New-Object System.Drawing.Size(420, 24)
    try { $txtName.Text = (& git config --global user.name 2>$null) } catch { }

    $lblEmail = New-Object System.Windows.Forms.Label
    $lblEmail.Text = 'user.email（如 name@example.com）'
    $lblEmail.Location = New-Object System.Drawing.Point(16, 74)
    $lblEmail.AutoSize = $true
    $txtEmail = New-Object System.Windows.Forms.TextBox
    $txtEmail.Location = New-Object System.Drawing.Point(16, 100)
    $txtEmail.Size = New-Object System.Drawing.Size(420, 24)
    try { $txtEmail.Text = (& git config --global user.email 2>$null) } catch { }

    $f.Controls.AddRange(@($lblName, $txtName, $lblEmail, $txtEmail))
    New-DialogButtons $f 160 '保存'
    $null = $f.ShowDialog()
    if ($f.DialogResult -eq [System.Windows.Forms.DialogResult]::OK) {
        $name = $txtName.Text.Trim()
        $email = $txtEmail.Text.Trim()
        if ($name) { $null = Invoke-Git @('config', '--global', 'user.name', $name) }
        if ($email) { $null = Invoke-Git @('config', '--global', 'user.email', $email) }
        Append-Log ("✅ 全局 Git 配置已更新：name=$name  email=$email") 'OK'
    }
    $f.Dispose()
}

function Show-CommitForm {
    param([string]$SuggestedType)
    $f = New-FormDialog '提交代码（Conventional Commits）' 480 280
    $lblType = New-Object System.Windows.Forms.Label
    $lblType.Text = '提交类型'
    $lblType.Location = New-Object System.Drawing.Point(16, 14)
    $lblType.AutoSize = $true
    $combo = New-Object System.Windows.Forms.ComboBox
    $combo.DropDownStyle = 'DropDownList'
    $combo.Location = New-Object System.Drawing.Point(16, 38)
    $combo.Size = New-Object System.Drawing.Size(200, 24)
    @('feat', 'fix', 'docs', 'style', 'refactor', 'perf', 'test', 'chore', 'ci') | ForEach-Object { $null = $combo.Items.Add($_) }
    if ($SuggestedType -and $combo.Items.Contains($SuggestedType)) { $combo.SelectedItem = $SuggestedType } else { $combo.SelectedIndex = 0 }

    $lblMsg = New-Object System.Windows.Forms.Label
    $lblMsg.Text = '简短描述（如 增加插件版本显示）'
    $lblMsg.Location = New-Object System.Drawing.Point(230, 14)
    $lblMsg.AutoSize = $true
    $txt = New-Object System.Windows.Forms.TextBox
    $txt.Location = New-Object System.Drawing.Point(230, 38)
    $txt.Size = New-Object System.Drawing.Size(228, 24)

    $hint = New-Object System.Windows.Forms.Label
    $hint.Text = "规范：feat 新功能 / fix 修复 / docs 文档 / style 格式`n      refactor 重构 / perf 性能 / test 测试 / chore 构建杂项 / ci CI`n示例：feat: 增加插件版本显示"
    $hint.Location = New-Object System.Drawing.Point(16, 74)
    $hint.Size = New-Object System.Drawing.Size(444, 60)
    $hint.ForeColor = [System.Drawing.Color]::DimGray

    $f.Controls.AddRange(@($lblType, $combo, $lblMsg, $txt, $hint))
    New-DialogButtons $f 180 '提交'
    $null = $f.ShowDialog()
    $result = $null
    if ($f.DialogResult -eq [System.Windows.Forms.DialogResult]::OK) {
        $msg = $txt.Text.Trim()
        if (-not $msg) {
            Show-Message '提交信息为空' '请填写提交描述后再提交。' 'Warning'
        } else {
            $result = @{ Type = [string]$combo.SelectedItem; Message = $msg }
        }
    }
    $f.Dispose()
    return $result
}

function Show-BranchCreateForm {
    $f = New-FormDialog '创建功能分支' 440 220
    $rbFeat = New-Object System.Windows.Forms.RadioButton
    $rbFeat.Text = '新功能 (feature)'
    $rbFeat.Location = New-Object System.Drawing.Point(16, 16)
    $rbFeat.AutoSize = $true
    $rbFeat.Checked = $true
    $rbFix = New-Object System.Windows.Forms.RadioButton
    $rbFix.Text = '修复 Bug (fix)'
    $rbFix.Location = New-Object System.Drawing.Point(180, 16)
    $rbFix.AutoSize = $true

    $lbl = New-Object System.Windows.Forms.Label
    $lbl.Text = '简短名称（如 add login page → feature/add-login-page）'
    $lbl.Location = New-Object System.Drawing.Point(16, 50)
    $lbl.AutoSize = $true
    $txt = New-Object System.Windows.Forms.TextBox
    $txt.Location = New-Object System.Drawing.Point(16, 78)
    $txt.Size = New-Object System.Drawing.Size(404, 24)

    $f.Controls.AddRange(@($rbFeat, $rbFix, $lbl, $txt))
    New-DialogButtons $f 140 '创建'
    $null = $f.ShowDialog()
    $branch = $null
    if ($f.DialogResult -eq [System.Windows.Forms.DialogResult]::OK) {
        $type = if ($rbFix.Checked) { 'fix' } else { 'feature' }
        $branch = New-BranchName $type $txt.Text
        if (-not $branch) {
            Show-Message '分支名称无效' '名称清洗后为空，请重新输入（仅字母数字，空格自动转为 -）。' 'Warning'
        }
    }
    $f.Dispose()
    return $branch
}

function Show-VersionForm {
    param([string]$Current)
    $f = New-FormDialog '发布新版本' 400 180
    $lbl = New-Object System.Windows.Forms.Label
    $lbl.Text = "当前版本：$Current`n请输入新版本号（如 1.2.0）："
    $lbl.Location = New-Object System.Drawing.Point(16, 14)
    $lbl.Size = New-Object System.Drawing.Size(360, 40)
    $txt = New-Object System.Windows.Forms.TextBox
    $txt.Location = New-Object System.Drawing.Point(16, 62)
    $txt.Size = New-Object System.Drawing.Size(360, 24)
    $f.Controls.Add($lbl)
    $f.Controls.Add($txt)
    New-DialogButtons $f 108 '下一步'
    $null = $f.ShowDialog()
    $ver = $null
    if ($f.DialogResult -eq [System.Windows.Forms.DialogResult]::OK) {
        $v = $txt.Text.Trim()
        if ($v -match '^\d+\.\d+\.\d+$') { $ver = $v }
        else { Show-Message '版本号格式错误' '请按 主.次.修订 格式输入，如 1.2.0。' 'Warning' }
    }
    $f.Dispose()
    return $ver
}

function Show-InputForm {
    param([string]$Title, [string]$Prompt, [string]$Default = '')
    $f = New-FormDialog $Title 420 170
    $lbl = New-Object System.Windows.Forms.Label
    $lbl.Text = $Prompt
    $lbl.Location = New-Object System.Drawing.Point(16, 14)
    $lbl.Size = New-Object System.Drawing.Size(384, 44)
    $txt = New-Object System.Windows.Forms.TextBox
    $txt.Location = New-Object System.Drawing.Point(16, 66)
    $txt.Size = New-Object System.Drawing.Size(384, 24)
    $txt.Text = $Default
    $f.Controls.Add($lbl)
    $f.Controls.Add($txt)
    New-DialogButtons $f 110 '确定'
    $null = $f.ShowDialog()
    $value = $null
    if ($f.DialogResult -eq [System.Windows.Forms.DialogResult]::OK) {
        $value = $txt.Text.Trim()
    }
    $f.Dispose()
    return $value
}

# ============================================================
#  操作实现
# ============================================================

# ------- 选择性提交：文件清单解析与勾选对话框 -------

# 将 git status --porcelain 的输出行解析为携带类别标记的文件列表（纯函数，便于测试）。
function ConvertFrom-GitStatusLines {
    param([string[]]$Lines)
    $items = @()
    foreach ($line in $Lines) {
        if (-not $line) { continue }
        $code = if ($line.Length -ge 2) { $line.Substring(0, 2) } else { '' }
        $path = if ($line.Length -gt 3) { $line.Substring(3) } else { '' }
        # 重命名/复制路径：`old -> new`，取末段新路径
        if ($path -match '\s+->\s+') {
            $parts = $path -split '\s+->\s+'
            $path = $parts[$parts.Count - 1].Trim()
        }
        # 含空格的路径会被 git 加引号，去除首尾引号
        if ($path.Length -ge 2 -and $path.StartsWith('"') -and $path.EndsWith('"')) {
            $path = $path.Substring(1, $path.Length - 2)
        }
        if (-not $path) { continue }
        $state = if ($code -eq '??') { '新增' }
                 elseif ($code.IndexOf('A') -ge 0 -or $code.IndexOf('?') -ge 0) { '新增' }
                 elseif ($code.IndexOf('D') -ge 0) { '删除' }
                 elseif ($code.IndexOf('R') -ge 0 -or $code.IndexOf('C') -ge 0) { '重命名' }
                 else { '修改' }
        $items += [pscustomobject]@{ Path = $path; Label = ('{0}  {1}' -f $state, $path) }
    }
    return $items
}

# 解析 git status --porcelain，返回带类别标记的文件列表（含新增/修改/删除/重命名）。
function Get-GitStatusFiles {
    $git = Get-GitExecutable
    if (-not $git) { return @() }
    $raw = & $git -C $script:RepoRoot -c core.quotepath=false status --porcelain 2>$null
    return ConvertFrom-GitStatusLines $raw
}

# 文件选择对话框：勾选要提交的文件（默认全选），返回所选路径数组；取消返回 $null。
function Show-StageForm {
    param([System.Object[]]$Items)
    $f = New-FormDialog '选择要提交的文件' 620 540
    $hint = New-Object System.Windows.Forms.Label
    $hint.Text = '勾选要纳入本次提交的文件（默认全选，可取消部分以「选择性提交」）。`n新增文件显示为 新增/??，修改/删除/重命名同列。未勾选的会保留在工作区，可下次再提交。'
    $hint.Location = New-Object System.Drawing.Point(14, 10)
    $hint.Size = New-Object System.Drawing.Size(586, 40)

    $cl = New-Object System.Windows.Forms.CheckedListBox
    $cl.Location = New-Object System.Drawing.Point(14, 56)
    $cl.Size = New-Object System.Drawing.Size(586, 402)
    $cl.CheckOnClick = $true
    $cl.DisplayMember = 'Label'
    try { $cl.Font = New-Object System.Drawing.Font('Consolas', 10) } catch { }
    foreach ($it in $Items) { $null = $cl.Items.Add($it, $true) }

    $btnAll = New-Object System.Windows.Forms.Button
    $btnAll.Text = '全选'
    $btnAll.Size = New-Object System.Drawing.Size(70, 28)
    $btnAll.Location = New-Object System.Drawing.Point(14, 468)
    $btnAll.add_Click({ for ($i = 0; $i -lt $cl.Items.Count; $i++) { $cl.SetItemChecked($i, $true) } })
    $btnNone = New-Object System.Windows.Forms.Button
    $btnNone.Text = '全不选'
    $btnNone.Size = New-Object System.Drawing.Size(70, 28)
    $btnNone.Location = New-Object System.Drawing.Point(90, 468)
    $btnNone.add_Click({ for ($i = 0; $i -lt $cl.Items.Count; $i++) { $cl.SetItemChecked($i, $false) } })

    $btnOk = New-Object System.Windows.Forms.Button
    $btnOk.Text = '确定'
    $btnOk.Size = New-Object System.Drawing.Size(90, 30)
    $btnOk.Location = New-Object System.Drawing.Point(($f.ClientSize.Width - 200), 466)
    $btnOk.DialogResult = 'OK'
    $btnCancel = New-Object System.Windows.Forms.Button
    $btnCancel.Text = '取消'
    $btnCancel.Size = New-Object System.Drawing.Size(90, 30)
    $btnCancel.Location = New-Object System.Drawing.Point(($f.ClientSize.Width - 104), 466)
    $btnCancel.DialogResult = 'Cancel'

    $f.Controls.AddRange(@($hint, $cl, $btnAll, $btnNone, $btnOk, $btnCancel))
    $f.AcceptButton = $btnOk
    $f.CancelButton = $btnCancel
    $null = $f.ShowDialog()
    $sel = @()
    if ($f.DialogResult -eq [System.Windows.Forms.DialogResult]::OK) {
        foreach ($it in $cl.CheckedItems) { $sel += $it.Path }
        $f.Dispose()
        return $sel
    }
    $f.Dispose()
    return $null
}

$script:ActInstall = {
    if (-not (Assert-Env @('node', 'pnpm'))) { return }
    Set-Busy $true
    try { $c = Invoke-Pnpm @('install'); Show-Result '安装依赖' $c }
    finally { Set-Busy $false; Refresh-Status }
}

$script:ActDev = {
    if (-not (Assert-Env @('node', 'pnpm', 'rust', 'vscpp'))) { return }
    Append-Log '提示：pnpm tauri dev 为长驻进程，点击「停止当前任务」或关闭窗口即可结束。' 'WARN'
    Set-Busy $true
    try {
        $c = Invoke-Pnpm @('tauri', 'dev')
        if ($c -eq -1) { Append-Log 'ℹ 本地开发已停止。' }
        elseif ($c -eq 0) { Show-Result '本地开发运行' 0 }
        else { Show-Result '本地开发运行' $c }
    } finally { Set-Busy $false; Refresh-Status }
}

$script:ActBuild = {
    if (-not (Assert-Env @('node', 'pnpm', 'rust', 'vscpp'))) { return }
    Set-Busy $true
    try { $c = Invoke-Pnpm @('tauri', 'build'); Show-Result '构建安装程序' $c }
    finally { Set-Busy $false; Refresh-Status }
}

$script:ActBuildPortable = {
    if (-not (Assert-Env @('node', 'pnpm', 'rust', 'vscpp'))) { return }
    $light = Confirm-Message '构建便携版程序' (
        "请选择运行时模式：`n`n· 是(Y) = 轻量（`pnpm portable --no-runtime --no-zip`，体积小；首次安装时自动下载运行环境）`n· 否(N) = 内置运行时（`pnpm portable --no-zip`，无需联网但体积大）`n`n只会构建 portable 目录（exe + 运行时 + README，输出到 dist-portable），不生成 zip。")
    Set-Busy $true
    try {
        $args = if ($light) { @('portable', '--no-runtime', '--no-zip') } else { @('portable', '--no-zip') }
        $mode = $(if ($light) { '轻量（--no-runtime）' } else { '内置运行时' })
        $c = Invoke-Pnpm $args
        Show-Result ("构建便携版程序（$mode）") $c
    }
    finally { Set-Busy $false; Refresh-Status }
}

$script:ActPortable = {
    if (-not (Assert-Env @('node', 'pnpm', 'rust', 'vscpp'))) { return }
    $light = Confirm-Message '打包便携版' (
        "请选择打包模式：`n`n· 是(Y) = 轻量 zip（`pnpm portable --no-runtime`，体积小；首次安装时自动下载运行环境）`n· 否(N) = 内置运行时 zip（`pnpm portable`，无需联网但体积大）")
    Set-Busy $true
    try {
        $args = if ($light) { @('portable', '--no-runtime') } else { @('portable') }
        $mode = $(if ($light) { '轻量（--no-runtime）' } else { '内置运行时' })
        $c = Invoke-Pnpm $args
        Show-Result ("打包便携版（$mode）") $c
    }
    finally { Set-Busy $false; Refresh-Status }
}

$script:ActGitConfig = {
    if (-not (Assert-Env @('git'))) { return }
    Show-GitConfigForm
}

$script:ActClone = {
    if (-not (Assert-Env @('git'))) { return }
    $dlg = New-Object System.Windows.Forms.FolderBrowserDialog
    $dlg.Description = '选择项目保存目录（将创建 dsh-control-panel 子目录）'
    if ($dlg.ShowDialog() -ne [System.Windows.Forms.DialogResult]::OK) { return }
    $target = Join-Path $dlg.SelectedPath 'dsh-control-panel'
    if (Test-Path $target) {
        Show-Message '目录已存在' "目标目录已存在：`n$target`n请选择其他保存目录，或直接进入该目录运行 maintain.bat。" 'Warning'
        return
    }
    Append-Log ("正在克隆到：$target")
    Set-Busy $true
    try {
        $c = Invoke-Git @('clone', $script:RepoUrl, $target) -WorkingDirectory $dlg.SelectedPath
        if ($c -eq 0) {
            Append-Log ("克隆完成：cd $target 后运行 maintain.bat")
            Show-Message '克隆完成' ("项目已克隆到：`n$target`n`n进入该目录后运行 maintain.bat 即可使用本工具。")
            Show-Result '克隆项目' $c
        } else {
            Show-Result '克隆项目' $c
        }
    } finally { Set-Busy $false; Refresh-Status }
}

$script:ActSyncMain = {
    if (-not (Assert-Env @('git'))) { return }
    if (-not (Assert-Repo)) { return }
    if (-not (Confirm-CleanOrContinue '同步主分支')) { return }
    Set-Busy $true
    try {
        $c1 = Invoke-Git @('checkout', 'main')
        if ($c1 -ne 0) {
            Show-Result '切换到主分支' $c1
            Append-Log '可能因本地改动与主分支冲突。' 'ERROR'
            return
        }
        $c2 = Invoke-Git @('pull', '--ff-only')
        Show-Result '同步主分支 main' $c2
    } finally { Set-Busy $false; Refresh-Status }
}

$script:ActCommit = {
    if (-not (Assert-Env @('git'))) { return }
    if (-not (Assert-Repo)) { return }
    $branch = Get-CurrentBranch
    if ($branch -eq 'HEAD' -or $branch -eq '—') {
        Show-Message '当前不在有效分支' '当前处于 detached HEAD 或无法识别分支，请先切换到具体分支再提交。' 'Warning'
        return
    }
    # 1) 列出工作区变更（含新增文件），让用户勾选要提交的文件
    $items = Get-GitStatusFiles
    if (-not $items -or $items.Count -eq 0) {
        Append-Log '没有需要提交的改动（工作区干净）。'
        Show-Message '无改动' '当前工作区没有需要提交的文件。' 'Info'
        return
    }
    $selected = Show-StageForm $items
    if ($null -eq $selected) { return }   # 用户取消
    if ($selected.Count -eq 0) {
        Show-Message '未选择文件' '请在列表中选择至少一个文件后再提交。' 'Warning'
        return
    }
    # 2) 填写提交信息
    $suggest = if ($branch -like 'fix/*') { 'fix' } elseif ($branch -like 'feature/*') { 'feat' } else { 'feat' }
    $r = Show-CommitForm $suggest
    if (-not $r) { return }
    # 3) 只暂存所选文件并提交（未勾选的保留在工作区）
    Set-Busy $true
    try {
        $null = Invoke-Git (@('add', '-A', '--') + $selected)
        Append-Log ("已暂存 {0} 个文件，开始提交…" -f $selected.Count)
        $c = Invoke-Git @('commit', '-m', ($r.Type + ': ' + $r.Message))
        Show-Result '提交代码' $c
        if ($c -eq 0 -and (Confirm-Message '推送到远程？' "是否将提交推送到 origin/$branch ？")) {
            $pc = Invoke-Git @('push', 'origin', $branch)
            if ($pc -eq 0) { Append-Log ("✅ 已推送到 origin/{0}" -f $branch) 'OK' }
            else { Show-Result ("推送 origin/{0}" -f $branch) $pc }
        }
    } finally { Set-Busy $false; Refresh-Status }
}

$script:ActCreateBranch = {
    if (-not (Assert-Env @('git'))) { return }
    if (-not (Assert-Repo)) { return }
    $branch = Show-BranchCreateForm
    if (-not $branch) { return }
    # 功能分支应从最新主分支创建：提示是否有未提交改动，然后切到 main 并拉取，再基于它创建分支。
    if (-not (Confirm-CleanOrContinue '创建功能分支')) { return }
    Set-Busy $true
    try {
        $cur = Get-CurrentBranch
        if ($cur -ne 'main') {
            $c = Invoke-Git @('checkout', 'main')
            if ($c -ne 0) {
                Show-Result '切换到主分支' $c
                Append-Log '可能因本地改动与主分支冲突。' 'ERROR'
                return
            }
        }
        $pc = Invoke-Git @('pull', '--ff-only')   # 先同步最新 main
        if ($pc -ne 0) { Show-Result '同步主分支' $pc; Append-Log '创建功能分支前同步 main 失败，已中止。' 'ERROR'; return }
        $c = Invoke-Git @('checkout', '-b', $branch)
        if ($c -eq 0) {
            Append-Log ("✅ 已基于最新 main 创建并切换到功能分支：$branch") 'OK'
            Refresh-Status
            Show-Result '创建功能分支' 0
        } else {
            Show-Result '创建功能分支' $c
        }
    } finally { Set-Busy $false; Refresh-Status }
}

$script:ActPushBranch = {
    if (-not (Assert-Env @('git'))) { return }
    if (-not (Assert-Repo)) { return }
    $branch = Get-CurrentBranch
    if ($branch -eq 'main' -or $branch -eq '—') {
        Show-Message '当前在主分支' '请先创建/切换到功能分支再推送（功能分支开发 → 创建功能分支）。' 'Warning'
        return
    }
    if ($branch -eq 'HEAD') {
        Show-Message '当前在 detached HEAD' '当前处于 detached HEAD，无法按分支推送。请先切到一个具体功能分支。' 'Warning'
        return
    }
    Set-Busy $true
    try {
        $c = Invoke-Git @('push', '-u', 'origin', $branch)
        if ($c -eq 0) {
            Append-Log ("✅ 已推送到 origin/{0}" -f $branch) 'OK'
            $prUrl = "$script:RepoWeb/compare/main...$branch?expand=1"
            Append-Log ("PR 创建页：$prUrl")
            Open-Url $prUrl
            Show-Message '已推送功能分支' (
                "分支 $branch 已推送到 GitHub。`n`n下一步：`n 1) 在打开的 PR 页面创建 Pull Request（base: main ← compare: $branch）`n 2) 等待 CI 通过并完成 Review / Merge`n 3) 回到本工具选择「同步合并后的 main」并「删除已合并的本地分支」")
        } else {
            Show-Result ("推送 origin/{0}" -f $branch) $c
        }
    } finally { Set-Busy $false; Refresh-Status }
}

$script:ActSyncMerged = {
    if (-not (Assert-Env @('git'))) { return }
    if (-not (Assert-Repo)) { return }
    if (-not (Confirm-CleanOrContinue '同步合并后的 main')) { return }
    Set-Busy $true
    try {
        $c1 = Invoke-Git @('checkout', 'main')
        if ($c1 -ne 0) {
            Show-Result '切换到主分支' $c1
            Append-Log '可能因本地改动冲突。' 'ERROR'
            return
        }
        $c2 = Invoke-Git @('pull', '--ff-only')
        if ($c2 -eq 0) { Append-Log '✅ 已同步合并后的 main（可再选择「删除已合并的本地分支」清理）。' 'OK' }
        else { Show-Result '同步合并后的 main' $c2 }
    } finally { Set-Busy $false; Refresh-Status }
}

$script:ActDeleteBranch = {
    if (-not (Assert-Env @('git'))) { return }
    if (-not (Assert-Repo)) { return }
    # 删除分支前应位于主分支：git branch -d 无法删除当前所在分支。
    $cur = Get-CurrentBranch
    if ($cur -ne 'main' -and $cur -ne '—') {
        if (-not (Confirm-CleanOrContinue '删除已合并分支')) { return }
        Set-Busy $true
        try { $c1 = Invoke-Git @('checkout', 'main') } finally { Set-Busy $false }
        if ($c1 -ne 0) {
            Append-Log '切换到主分支失败（可能因本地改动冲突）。' 'ERROR'
            return
        }
        Refresh-Status
    }
    $git = Get-GitExecutable
    $merged = if ($git) {
        (& $git -C $script:RepoRoot branch --merged main 2>$null) |
            ForEach-Object { $_.Trim() -replace '^\*\s*', '' } |
            Where-Object { $_ -and $_ -ne 'main' }
    } else { @() }
    $prompt = if ($merged) {
        "以下分支已合并到 main，可安全删除：`n" + ($merged -join "`n") + "`n`n请输入要删除的分支名："
    } else {
        '未发现已合并到 main 的分支。请输入要删除的分支名（未合并分支请先在远端合并后再删）：'
    }
    $name = Show-InputForm '删除已合并的本地分支' $prompt ''
    if (-not $name) { return }
    if ($name -eq (Get-CurrentBranch)) {
        Show-Message '不能删除当前分支' '不能删除当前所在分支，请先切换到其他分支。' 'Warning'
        return
    }
    Set-Busy $true
    try {
        $c = Invoke-Git @('branch', '-d', $name)
        if ($c -eq 0) { Show-Result ("删除分支 {0}" -f $name) 0 }
        else {
            Show-Result ("删除分支 {0}" -f $name) $c
            Append-Log '提示：git branch -d 仅删除已合并分支；若确认要强制删除未合并分支，请手动执行 git branch -D。' 'WARN'
        }
    } finally { Set-Busy $false; Refresh-Status }
}

# 判断 tag 是否已存在（本地或远端）。
function Test-TagExists {
    param([string]$Tag)
    $git = Get-GitExecutable
    if (-not $git) { return $false }
    $local = (& $git -C $script:RepoRoot rev-parse -q --verify "refs/tags/$Tag" 2>$null)
    if ($local) { return $true }
    $remote = (& $git -C $script:RepoRoot ls-remote --tags origin "refs/tags/$Tag" 2>$null)
    if ($remote) { return $true }
    return $false
}

$script:ActRelease = {
    if (-not (Assert-Env @('git'))) { return }
    if (-not (Assert-Repo)) { return }
    # 发布必须在 main 分支执行。
    $branch = Get-CurrentBranch
    if ($branch -ne 'main') {
        Show-Message '不在主分支' ('发布新版本必须在 main 分支执行。`n当前分支：' + $branch + '`n请先「同步主分支」后再发布。') 'Warning'
        return
    }
    if ($branch -eq 'HEAD') {
        Show-Message '在 detached HEAD' '当前处于 detached HEAD，无法发布。请先切到 main。' 'Warning'
        return
    }
    $current = Get-CurrentVersion
    $new = Show-VersionForm $current
    if (-not $new) { return }
    if ($new -eq $current) {
        Show-Message '版本号未变化' '新版本号与当前版本相同，无需发布。' 'Warning'
        return
    }
    if (Test-TagExists "v$new") {
        Show-Message 'tag 已存在' ("远端/本地已存在 tag v$new。`n请确认：`n  · 是否已发布过该版本；`n  · 或换一个版本号。") 'Warning'
        return
    }
    # 工作区有未提交改动：发布将 git add -A 一并提交，需先告知。
    if (-not (Confirm-CleanOrContinue '发布新版本')) { return }
    # 发布预览 + 确认（在修改任何文件之前，取消则无副作用）。
    $msg = @"
将执行（触发 GitHub Actions 自动打包发布）：
  1) 修改版本号：$current -> $new （package.json / tauri.conf.json / Cargo.toml / Cargo.lock / constants.ts）
  2) git add -A
  3) git commit -m "chore: release v$new"
  4) git push origin main
  5) git tag v$new
  6) git push origin v$new

  GitHub Actions 将打包发布**轻量便携版**（pnpm portable --no-runtime，
  不含 Node.js/pnpm 运行时，首次安装时自动下载；体积更小）。

确认继续？
"@
    if (-not (Confirm-Message '确认发布新版本？' $msg)) { return }
    Set-Busy $true
    try {
        Set-VersionAll $new
        Refresh-Status
        Append-Log '--- 已修改文件（git status）---'
        $null = Invoke-Git @('status', '--short')
        $null = Invoke-Git @('add', '-A')
        $c = Invoke-Git @('commit', '-m', ("chore: release v$new"))
        if ($c -ne 0) { Show-Result '发布提交' $c; Append-Log '发布已中止（可先处理问题后再试）。' 'ERROR'; return }
        Show-Result '发布提交（版本号修改 + 提交）' 0
        $pc = Invoke-Git @('push', 'origin', 'main')
        if ($pc -ne 0) { Show-Result '推送 main' $pc; Append-Log '发布已中止（不会打 tag）。' 'ERROR'; return }
        Append-Log '✅ 已推送到 origin/main' 'OK'
        $null = Invoke-Git @('tag', "v$new")
        $tc = Invoke-Git @('push', 'origin', "v$new")
        if ($tc -eq 0) {
            Append-Log ("✅ 已推送 tag v$new") 'OK'
            Open-Url $script:RepoActions
            Show-Message '已触发发布工作流' (
                "已推送 tag v$new，GitHub Actions 的 Release (portable zip) 已开始执行。`n`n进度查看：`n$script:RepoActions`n`n完成后 zip 下载：`n$script:RepoReleases")
        } else {
            Show-Result ("推送 tag v$new") $tc
            Append-Log '请检查远端是否已存在该 tag 或网络问题。' 'ERROR'
        }
    } catch {
        Append-Log ("❌ 发布出错：" + $_.Exception.Message) 'ERROR'
        Show-Message '发布出错' ("发生错误：`n$($_.Exception.Message)`n`n提示：若版本号文件已被部分修改，可用 `git checkout -- .` 恢复。") 'Error'
    } finally { Set-Busy $false; Refresh-Status }
}

$script:ActEnvCheck = {
    Set-Busy $true
    try {
        Append-Log '--- 环境依赖自检 ---'
        $ok = Assert-Env @('git', 'node', 'pnpm', 'rust', 'vscpp')
        if ($ok) {
            Append-Log '✅ 环境检查完成，依赖齐全。' 'OK'
        } else {
            Append-Log '❌ 环境存在缺失或版本过低项，请按上方指引安装。' 'WARN'
        }
    } finally { Set-Busy $false }
}

$script:ActStop = {
    $script:StopRequested = $true
    Append-Log '正在停止当前任务…' 'WARN'
}

$script:ActRefresh = {
    Refresh-Status
    Append-Log '状态已刷新。'
}

# ============================================================
#  界面构建
# ============================================================

function New-MenuButton {
    param([string]$Text, [scriptblock]$Action)
    $b = New-Object System.Windows.Forms.Button
    $b.Text = $Text
    $b.Width = 320
    $b.Height = 40
    $b.FlatStyle = 'Flat'
    $b.FlatAppearance.BorderSize = 1
    $b.FlatAppearance.BorderColor = [System.Drawing.Color]::FromArgb(210, 216, 224)
    $b.FlatAppearance.MouseOverBackColor = [System.Drawing.Color]::FromArgb(227, 237, 250)
    $b.FlatAppearance.MouseDownBackColor = [System.Drawing.Color]::FromArgb(205, 222, 240)
    $b.BackColor = [System.Drawing.Color]::White
    $b.TextAlign = 'MiddleLeft'
    $b.Margin = New-Object System.Windows.Forms.Padding(2, 5, 2, 5)
    $b.Tag = $Action
    $b.add_Click({
        param($sender, $e)
        $act = $sender.Tag
        try { & $act }
        catch {
            Append-Log ("操作出错：" + $_.Exception.Message) 'ERROR'
            Show-Message '操作出错' $_.Exception.Message 'Error'
        }
    })
    return $b
}

function New-GroupLabel {
    param([string]$Text)
    $l = New-Object System.Windows.Forms.Label
    $l.Text = $Text
    $l.Width = 320
    $l.Height = 22
    $l.Margin = New-Object System.Windows.Forms.Padding(0, 12, 0, 4)
    try { $l.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 9, [System.Drawing.FontStyle]::Bold) } catch { }
    $l.ForeColor = [System.Drawing.Color]::FromArgb(43, 58, 74)
    return $l
}

# 创建一个分类标签页：简介 + 一组按钮（纵向排列、可滚动）。
function New-TabPage {
    param([string]$Title, [string]$Desc, [System.Object[]]$Buttons)
    $page = New-Object System.Windows.Forms.TabPage
    $page.Text = $Title
    $page.BackColor = [System.Drawing.Color]::FromArgb(245, 247, 250)
    $flow = New-Object System.Windows.Forms.FlowLayoutPanel
    $flow.Dock = 'Fill'
    $flow.FlowDirection = 'TopDown'
    $flow.WrapContents = $false
    $flow.AutoScroll = $true
    $flow.Padding = New-Object System.Windows.Forms.Padding(10, 12, 10, 10)
    $lblDesc = New-Object System.Windows.Forms.Label
    $lblDesc.Text = $Desc
    $lblDesc.Width = 324
    $lblDesc.Height = 44
    $lblDesc.AutoSize = $false
    $lblDesc.ForeColor = [System.Drawing.Color]::FromArgb(110, 120, 132)
    $flow.Controls.Add($lblDesc)
    foreach ($btn in $Buttons) { $flow.Controls.Add($btn) }
    $page.Controls.Add($flow)
    return $page
}

function Build-Form {
    $form = New-Object System.Windows.Forms.Form
    $form.Text = 'DSH Control Panel · 项目维护工具'
    $form.Size = New-Object System.Drawing.Size(1140, 780)
    $form.MinimumSize = New-Object System.Drawing.Size(940, 660)
    $form.StartPosition = 'CenterScreen'
    try { $form.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 9) } catch { }
    $form.BackColor = [System.Drawing.Color]::FromArgb(245, 247, 250)

    $form.add_FormClosing({
        $script:StopRequested = $true   # 优雅停止正在运行的泵循环
        if ($script:CurrentProc) {
            try {
                & "$env:SystemRoot\System32\taskkill.exe" /PID $script:CurrentProc.Id /T /F 2>$null | Out-Null
            } catch { }
        }
    })

    # --- 日志框（Fill，RichTextBox 支持按行着色） ---
    $log = New-Object System.Windows.Forms.RichTextBox
    $log.Dock = 'Fill'
    $log.Multiline = $true
    $log.ReadOnly = $true
    $log.ScrollBars = 'Vertical'
    $log.WordWrap = $false
    $log.BackColor = [System.Drawing.Color]::FromArgb(28, 30, 34)
    $log.ForeColor = [System.Drawing.Color]::FromArgb(210, 215, 220)
    try { $log.Font = New-Object System.Drawing.Font('Consolas', 10) } catch { }
    $form.Controls.Add($log)

    # --- 左侧：分类 Tab + 底部操作条 ---
    $left = New-Object System.Windows.Forms.Panel
    $left.Dock = 'Left'
    $left.Width = 372
    $left.BackColor = [System.Drawing.Color]::FromArgb(245, 247, 250)

    $tab = New-Object System.Windows.Forms.TabControl
    $tab.Dock = 'Fill'
    try { $tab.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 9.5) } catch { }

    # 本地开发
    $btnInstall  = New-MenuButton '安装依赖  (pnpm install)' $script:ActInstall
    $btnDev      = New-MenuButton '本地开发运行  (pnpm tauri dev)' $script:ActDev
    $btnBuild    = New-MenuButton '构建安装程序  (pnpm tauri build)' $script:ActBuild
    $btnBuildPortable = New-MenuButton '构建便携版程序  (pnpm portable --no-zip)' $script:ActBuildPortable
    $btnPortable = New-MenuButton '打包便携版 zip  (pnpm portable[--no-runtime])' $script:ActPortable
    # 主分支开发
    $btnSyncMain = New-MenuButton '同步主分支  (checkout main + pull)' $script:ActSyncMain
    $btnCommitMain = New-MenuButton '提交代码（当前分支）' $script:ActCommit
    $btnRelease  = New-MenuButton '发布新版本  (版本号/tag/Release)' $script:ActRelease
    # 功能分支开发
    $btnCreateBr = New-MenuButton '创建功能分支  (feature/fix)' $script:ActCreateBranch
    $btnCommitBr = New-MenuButton '提交代码（当前分支）' $script:ActCommit
    $btnPushBr   = New-MenuButton '推送分支并打开 PR' $script:ActPushBranch
    $btnSyncMerged = New-MenuButton '同步合并后的 main' $script:ActSyncMerged
    $btnDelBr    = New-MenuButton '删除已合并的本地分支' $script:ActDeleteBranch
    # 工具 / 设置
    $btnClone    = New-MenuButton '克隆项目到本地' $script:ActClone
    $btnGitConfig= New-MenuButton '设置全局 Git 用户名/邮箱' $script:ActGitConfig
    $btnEnv      = New-MenuButton '环境依赖自检' $script:ActEnvCheck

    $pageDev = New-TabPage '本地开发' '安装依赖、运行、构建、打包。执行前会自动检查 Node / pnpm / Rust / VS C++ Build Tools；构建便携版程序/打包 zip 时可选择轻量（--no-runtime）或内置运行时，构建便携版程序只输出目录、不生成 zip。' @($btnInstall, $btnDev, $btnBuild, $btnBuildPortable, $btnPortable)
    $pageMain = New-TabPage '主分支开发' '直接在主分支上开发：同步、提交代码（提交基于当前分支，会询问是否推送）、发布新版本。' @($btnSyncMain, $btnCommitMain, $btnRelease)
    $pageBranch = New-TabPage '功能分支开发' '先创建功能分支，开发后提交代码，推送到 GitHub 并提交 PR；合并后回主分支同步并清理本地分支。' @($btnCreateBr, $btnCommitBr, $btnPushBr, $btnSyncMerged, $btnDelBr)
    $pageTool = New-TabPage '工具 / 设置' '克隆项目到本地、设置全局 Git 身份、环境依赖自检。' @($btnClone, $btnGitConfig, $btnEnv)
    $tab.TabPages.AddRange(@($pageDev, $pageMain, $pageBranch, $pageTool))
    $left.Controls.Add($tab)

    # 底部操作条：停止当前任务 / 刷新状态
    $bar = New-Object System.Windows.Forms.Panel
    $bar.Dock = 'Bottom'
    $bar.Height = 54
    $bar.BackColor = [System.Drawing.Color]::FromArgb(233, 236, 240)
    $btnStop = New-Object System.Windows.Forms.Button
    $btnStop.Text = '■ 停止当前任务'
    $btnStop.Width = 160
    $btnStop.Height = 36
    $btnStop.Location = New-Object System.Drawing.Point(12, 9)
    $btnStop.FlatStyle = 'Flat'
    $btnStop.FlatAppearance.BorderSize = 1
    $btnStop.FlatAppearance.BorderColor = [System.Drawing.Color]::FromArgb(220, 160, 160)
    $btnStop.FlatAppearance.MouseOverBackColor = [System.Drawing.Color]::FromArgb(255, 232, 232)
    $btnStop.BackColor = [System.Drawing.Color]::FromArgb(255, 245, 245)
    $btnStop.ForeColor = [System.Drawing.Color]::FromArgb(190, 40, 40)
    $btnStop.Enabled = $false
    $btnStop.Tag = $script:ActStop
    $btnStop.add_Click({ param($sender, $e) try { & $sender.Tag } catch { Show-Message '操作出错' $_.Exception.Message 'Error' } })
    $btnRefresh = New-Object System.Windows.Forms.Button
    $btnRefresh.Text = '刷新状态'
    $btnRefresh.Width = 100
    $btnRefresh.Height = 36
    $btnRefresh.Location = New-Object System.Drawing.Point(182, 9)
    $btnRefresh.Tag = $script:ActRefresh
    $btnRefresh.add_Click({ param($sender, $e) try { & $sender.Tag } catch { Show-Message '操作出错' $_.Exception.Message 'Error' } })
    $bar.Controls.AddRange(@($btnStop, $btnRefresh))
    $left.Controls.Add($bar)
    $form.Controls.Add($left)

    # --- 顶部标题栏（Dock=Top） ---
    $header = New-Object System.Windows.Forms.Panel
    $header.Dock = 'Top'
    $header.Height = 62
    $header.BackColor = [System.Drawing.Color]::FromArgb(43, 58, 74)
    $lblTitle = New-Object System.Windows.Forms.Label
    $lblTitle.Text = 'DSH Control Panel · 项目维护工具'
    $lblTitle.Location = New-Object System.Drawing.Point(16, 12)
    $lblTitle.AutoSize = $true
    try { $lblTitle.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 15, [System.Drawing.FontStyle]::Bold) } catch { }
    $lblTitle.ForeColor = [System.Drawing.Color]::White
    $lblSub = New-Object System.Windows.Forms.Label
    $lblSub.Text = '面向开发者的仓库维护 & 发版工具'
    $lblSub.Location = New-Object System.Drawing.Point(18, 40)
    $lblSub.AutoSize = $true
    $lblSub.ForeColor = [System.Drawing.Color]::FromArgb(170, 185, 200)
    $header.Controls.AddRange(@($lblTitle, $lblSub))

    # --- 状态栏（Dock=Top，位于标题栏下方） ---
    $status = New-Object System.Windows.Forms.FlowLayoutPanel
    $status.Dock = 'Top'
    $status.Height = 32
    $status.BackColor = [System.Drawing.Color]::FromArgb(233, 236, 240)
    $status.Padding = New-Object System.Windows.Forms.Padding(12, 8, 8, 0)
    $lblDir = New-Object System.Windows.Forms.Label
    $lblDir.AutoSize = $true
    $lblDir.ForeColor = [System.Drawing.Color]::FromArgb(70, 80, 92)
    $lblRepo = New-Object System.Windows.Forms.Label
    $lblRepo.AutoSize = $true
    $lblRepo.ForeColor = [System.Drawing.Color]::FromArgb(70, 80, 92)
    $lblBranch = New-Object System.Windows.Forms.Label
    $lblBranch.AutoSize = $true
    $lblBranch.ForeColor = [System.Drawing.Color]::FromArgb(70, 80, 92)
    $lblVersion = New-Object System.Windows.Forms.Label
    $lblVersion.AutoSize = $true
    $lblVersion.ForeColor = [System.Drawing.Color]::FromArgb(70, 80, 92)
    foreach ($l in @($lblDir, $lblRepo, $lblBranch, $lblVersion)) {
        $l.Margin = New-Object System.Windows.Forms.Padding(6, 0, 16, 0)
    }
    $status.Controls.AddRange(@($lblDir, $lblRepo, $lblBranch, $lblVersion))
    $form.Controls.Add($status)
    $form.Controls.Add($header)

    # --- 挂接全局引用 ---
    $script:LogBox = $log
    $script:MainForm = $form
    $script:LblDir = $lblDir
    $script:LblRepo = $lblRepo
    $script:LblBranch = $lblBranch
    $script:LblVersion = $lblVersion
    $script:BtnStop = $btnStop
    $script:BtnRefresh = $btnRefresh
    $script:ActionButtons = @($btnInstall, $btnDev, $btnBuild, $btnBuildPortable, $btnPortable, $btnSyncMain, $btnCommitMain,
        $btnRelease, $btnCreateBr, $btnCommitBr, $btnPushBr, $btnSyncMerged, $btnDelBr, $btnClone, $btnGitConfig, $btnEnv)

    # --- 日志刷新 Timer：UI 线程定时把队列内容写入日志框（避免跨线程更新控件），并按级别着色 ---
    $script:LogQueue = [System.Collections.Queue]::Synchronized([System.Collections.Queue]::new())
    $timer = New-Object System.Windows.Forms.Timer
    $timer.Interval = 120
    $timer.add_Tick({
        if (-not $script:LogBox -or $script:LogBox.IsDisposed) { return }
        try {
            $changed = $false
            while ($script:LogQueue.Count -gt 0) {
                $item = $script:LogQueue.Dequeue()
                # 按级别着色：OK 绿 / INFO 灰白 / WARN 琥珀 / ERROR 红 / CMD 蓝（执行的命令）
                switch ($item.Kind) {
                    'OK'    { $script:LogBox.SelectionColor = [System.Drawing.Color]::FromArgb(80, 200, 120) }
                    'WARN'  { $script:LogBox.SelectionColor = [System.Drawing.Color]::FromArgb(250, 173, 20) }
                    'ERROR' { $script:LogBox.SelectionColor = [System.Drawing.Color]::FromArgb(255, 92, 92) }
                    'CMD'   { $script:LogBox.SelectionColor = [System.Drawing.Color]::FromArgb(96, 165, 250) }
                    default { $script:LogBox.SelectionColor = [System.Drawing.Color]::FromArgb(210, 215, 220) }
                }
                $script:LogBox.AppendText($item.Text + [Environment]::NewLine)
                $changed = $true
            }
            # 复位默认色
            $script:LogBox.SelectionColor = [System.Drawing.Color]::FromArgb(210, 215, 220)
            if ($changed) {
                # 日志过长时截断，避免长期运行导致内存/性能下降
                if ($script:LogBox.TextLength -gt 500000) {
                    $keep = $script:LogBox.Text.Substring($script:LogBox.TextLength - 200000)
                    $script:LogBox.Clear()
                    $script:LogBox.SelectionColor = [System.Drawing.Color]::FromArgb(130, 150, 170)
                    $script:LogBox.AppendText('[LOG] 日志过长，已保留末尾内容。' + [Environment]::NewLine)
                    $script:LogBox.SelectionColor = [System.Drawing.Color]::FromArgb(210, 215, 220)
                    $script:LogBox.AppendText($keep)
                }
                $script:LogBox.SelectionStart = $script:LogBox.TextLength
                $script:LogBox.ScrollToCaret()
            }
        } catch { }
    })
    $script:LogTimer = $timer
    $timer.Start()

    return $form
}

# ============================================================
#  主入口
# ============================================================
[System.Windows.Forms.Application]::EnableVisualStyles()
[System.Windows.Forms.Application]::SetCompatibleTextRenderingDefault($false)

# 兜底：任何未捕获异常（含后台线程）以对话框呈现，避免静默闪退
$null = [System.AppDomain]::CurrentDomain.add_UnhandledException({
    param($s, $e)
    try {
        $ex = $e.ExceptionObject
        $null = [System.Windows.Forms.MessageBox]::Show(
            "发生未捕获异常：`n$ex", 'DSH Maintain',
            [System.Windows.Forms.MessageBoxButtons]::OK, [System.Windows.Forms.MessageBoxIcon]::Error)
    } catch { }
})
[System.Windows.Forms.Application]::add_ThreadException({
    param($s, $e)
    try {
        $null = [System.Windows.Forms.MessageBox]::Show(
            "界面线程异常：`n$($e.Exception)", 'DSH Maintain',
            [System.Windows.Forms.MessageBoxButtons]::OK, [System.Windows.Forms.MessageBoxIcon]::Error)
    } catch { }
})

$script:MainForm = Build-Form
Refresh-Status
Append-Log ("欢迎使用 DSH Control Panel 项目维护工具（PowerShell $($PSVersionTable.PSVersion)）")
Append-Log "项目目录：$script:RepoRoot"
if (-not (Get-RepoValid)) { Append-Log '注意：当前目录不是有效仓库，Git 与发版功能不可用。' 'WARN' }

[System.Windows.Forms.Application]::Run($script:MainForm)
