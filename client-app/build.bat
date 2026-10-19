@echo off
chcp 65001 >nul
cd /d "%~dp0"

echo ============================================
echo   编译并打包 client-app
echo ============================================
echo.

where mvn >nul 2>nul
if errorlevel 1 (
    echo [错误] 找不到 mvn 命令。
    echo 请确认 Maven 已安装并加入 PATH，或在 IDEA 的 Maven 面板里执行 package。
    pause
    exit /b 1
)

call mvn -B clean package -DskipTests
if errorlevel 1 (
    echo.
    echo [失败] 编译出错，请把上面的红色信息发给同学。
    pause
    exit /b 1
)

echo.
echo [成功] 产物：target\dbms-client.jar
echo 直接双击 run.bat 即可运行。
pause
