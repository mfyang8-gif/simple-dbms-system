@echo off
chcp 65001 >nul
cd /d "%~dp0"
echo ============================================
echo   MiniDB 管理控制台（课题1 - 数据库客户端）
echo ============================================
echo.

if not exist "target\dbms-client.jar" (
    echo [错误] 找不到 target\dbms-client.jar
    echo 请先编译打包：在本目录运行 mvn package
    pause
    exit /b 1
)

rem 必须用 JDK 17+ 启动：本工程 maven.compiler.release=17，
rem PATH 上若还是 Java 8 会直接抛 UnsupportedClassVersionError。
if "%JAVA_HOME%"=="" (
    echo [错误] 未设置 JAVA_HOME，无法确定 JDK 位置。
    echo 期望的 JDK 版本为 17 或更高（当前工程编译目标 17）。
    pause
    exit /b 1
)

echo 使用 JDK: %JAVA_HOME%
echo 正在启动，请稍候...
start "" "%JAVA_HOME%\bin\javaw" -Dfile.encoding=UTF-8 -cp "target\dbms-client.jar" com.dbms.client.console.ConsoleMain

echo 已启动。若窗口未出现，请检查是否设置了正确的 JAVA_HOME（17+）。
timeout /t 2 >nul
