@echo off
chcp 65001 > nul
title DashCam360 - 汽車環景魚眼校正與 3D 虛擬動態投影系統

echo ========================================================
echo   DashCam360 - 4 分割魚眼校正與 3D 環景案頭應用程式
echo ========================================================
echo.

set PYTHON_CMD=

:: 優先檢測具備 PySide6 的系統 Python 3.13
if exist "C:\Users\HPI\AppData\Local\Programs\Python\Python313\python.exe" (
    set PYTHON_CMD="C:\Users\HPI\AppData\Local\Programs\Python\Python313\python.exe"
) else (
    where python >nul 2>nul
    if %errorlevel% equ 0 (
        set PYTHON_CMD=python
    ) else (
        echo [錯誤] 找不到 Python 環境，請確認已安裝 Python 3.11 或 3.13。
        pause
        exit /b 1
    )
)

echo [啟動中] 使用直譯器: %PYTHON_CMD%
%PYTHON_CMD% main.py %*

if %errorlevel% neq 0 (
    echo.
    echo 若 PySide6 圖形視窗無法啟動，嘗試以網頁模式啟動:
    %PYTHON_CMD% main.py --web-only
)

pause
