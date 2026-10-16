@echo off
chcp 65001 >nul
cd /d "%~dp0"

set "JAVA_HOME=D:\java\jdk-17"
set "MVN=C:\Users\Administrator\.m2\wrapper\dists\apache-maven-3.9.15-bin\4rlcemksed9vjmkvgss0jpc4po\apache-maven-3.9.15\bin\mvn.cmd"

echo ============================================
echo   编译并打包 client-app
echo ============================================
echo.

if not exist "%MVN%" (
    echo [错误] 找不到 Maven: %MVN%
    echo 请在 IDEA 里用 Maven 面板重新加载项目。
    pause
    exit /b 1
)

call "%MVN%" -B clean package -DskipTests
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
