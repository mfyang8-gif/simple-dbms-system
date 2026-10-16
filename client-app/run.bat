@echo off
chcp 65001 >nul
cd /d "%~dp0"
echo ============================================
echo   简易仓库管理系统 - 客户端
echo ============================================
echo.

if not exist "target\dbms-client.jar" (
    echo [错误] 找不到 target\dbms-client.jar
    echo 请先编译：右键本目录空白处 -^> 在终端中打开 -^> 运行 build.bat
    pause
    exit /b 1
)

echo 正在启动，请稍候...
start "" javaw -Dfile.encoding=UTF-8 -jar "target\dbms-client.jar"

echo 已启动。若窗口未出现，请检查是否有报错。
timeout /t 2 >nul
