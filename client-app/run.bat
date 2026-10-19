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

rem ---- 自动升级：启动前应用上次下载的新版本 ----
rem Windows 下正在运行的 jar 无法被覆盖，所以必须在 java 启动之前替换。
if exist "updates\dbms-client.jar.new" (
    echo 发现待安装的更新，正在应用...
    copy /y "updates\dbms-client.jar.new" "target\dbms-client.jar" >nul 2>nul
    if errorlevel 1 (
        echo [提示] 替换失败（程序可能仍在运行），本次先用旧版本启动。
    ) else (
        del "updates\dbms-client.jar.new" >nul 2>nul
        echo 更新已应用，本次以新版本启动。
    )
    echo.
)

echo 正在启动，请稍候...
start "" javaw -Dfile.encoding=UTF-8 -jar "target\dbms-client.jar"

echo 已启动。若窗口未出现，请检查是否有报错。
timeout /t 2 >nul
