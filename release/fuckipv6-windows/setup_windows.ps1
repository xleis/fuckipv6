# fuckipv6 Windows 安装脚本
# 需要以管理员身份运行

param(
    [switch]$SkipWinTun = $false,
    [switch]$BuildOnly = $false
)

$ErrorActionPreference = "Stop"

Write-Host "=====================================" -ForegroundColor Cyan
Write-Host "fuckipv6 Windows 安装脚本" -ForegroundColor Cyan
Write-Host "=====================================" -ForegroundColor Cyan
Write-Host ""

# 检查管理员权限
$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Host "[错误] 需要管理员权限。请右键点击 PowerShell 并选择 '以管理员身份运行'" -ForegroundColor Red
    exit 1
}

Write-Host "[OK] 检测到管理员权限" -ForegroundColor Green

# 检查 Rust 工具链
Write-Host ""
Write-Host "检查 Rust 工具链..." -ForegroundColor Yellow
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "[错误] 未找到 cargo。请先安装 Rust: https://rustup.rs/" -ForegroundColor Red
    exit 1
}

$rustVersion = cargo --version
Write-Host "[OK] 已安装 Rust: $rustVersion" -ForegroundColor Green

# 下载 WinTun
if (-not $SkipWinTun) {
    Write-Host ""
    Write-Host "下载 WinTun 驱动..." -ForegroundColor Yellow

    $wintunVersion = "0.14.1"
    $wintunUrl = "https://www.wintun.net/builds/wintun-$wintunVersion.zip"
    $wintunZip = "wintun.zip"
    $wintunDir = "wintun"

    # 检查是否已存在 wintun.dll
    if (Test-Path "wintun.dll") {
        Write-Host "[OK] wintun.dll 已存在，跳过下载" -ForegroundColor Green
    } else {
        Write-Host "正在从 $wintunUrl 下载..." -ForegroundColor Cyan

        try {
            Invoke-WebRequest -Uri $wintunUrl -OutFile $wintunZip -UseBasicParsing
            Write-Host "[OK] 下载完成" -ForegroundColor Green
        } catch {
            Write-Host "[错误] 下载失败: $_" -ForegroundColor Red
            Write-Host "请手动下载 wintun.dll 并放到程序目录" -ForegroundColor Yellow
            exit 1
        }

        # 解压
        Write-Host "正在解压..." -ForegroundColor Cyan
        Expand-Archive -Path $wintunZip -DestinationPath $wintunDir -Force

        # 复制对应架构的 DLL
        $arch = if ([Environment]::Is64BitOperatingSystem) { "amd64" } else { "x86" }
        Copy-Item "$wintunDir\wintun\bin\$arch\wintun.dll" -Destination "." -Force

        # 清理
        Remove-Item $wintunZip -Force
        Remove-Item $wintunDir -Recurse -Force

        Write-Host "[OK] wintun.dll 已安装" -ForegroundColor Green
    }
}

# 构建项目
if (-not $BuildOnly) {
    Write-Host ""
    Write-Host "构建 fuckipv6..." -ForegroundColor Yellow

    cargo build --release

    if ($LASTEXITCODE -eq 0) {
        Write-Host "[OK] 构建成功" -ForegroundColor Green

        # 复制 wintun.dll 到 target/release
        if (Test-Path "wintun.dll") {
            Copy-Item "wintun.dll" -Destination "target\release\" -Force
            Write-Host "[OK] wintun.dll 已复制到 target\release\" -ForegroundColor Green
        }
    } else {
        Write-Host "[错误] 构建失败" -ForegroundColor Red
        exit 1
    }
}

# 安装说明
Write-Host ""
Write-Host "=====================================" -ForegroundColor Cyan
Write-Host "安装完成!" -ForegroundColor Green
Write-Host "=====================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "可执行文件位置: target\release\fuckipv6.exe" -ForegroundColor Yellow
Write-Host ""
Write-Host "使用方法:" -ForegroundColor Cyan
Write-Host "  1. 以管理员身份运行 PowerShell" -ForegroundColor White
Write-Host "  2. cd target\release" -ForegroundColor White
Write-Host "  3. .\fuckipv6.exe --help" -ForegroundColor White
Write-Host ""
Write-Host "快速开始:" -ForegroundColor Cyan
Write-Host "  # 生成默认配置" -ForegroundColor White
Write-Host "  .\fuckipv6.exe init-config" -ForegroundColor White
Write-Host ""
Write-Host "  # 启动客户端（需要先启动协调服务器）" -ForegroundColor White
Write-Host "  .\fuckipv6.exe start" -ForegroundColor White
Write-Host ""
Write-Host "注意事项:" -ForegroundColor Yellow
Write-Host "  - 运行客户端时必须使用管理员权限" -ForegroundColor White
Write-Host "  - wintun.dll 必须与 fuckipv6.exe 在同一目录" -ForegroundColor White
Write-Host "  - 确保防火墙允许 UDP 6666 端口" -ForegroundColor White
Write-Host ""
