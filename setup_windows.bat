@echo off
echo ====================================
echo fuckipv6 Windows 安装
echo ====================================
echo.
echo 正在以管理员权限启动...
echo.

powershell -ExecutionPolicy Bypass -File "%~dp0setup_windows.ps1"

if %ERRORLEVEL% NEQ 0 (
    echo.
    echo 安装失败，请检查上面的错误信息
    pause
    exit /b 1
)

echo.
pause
