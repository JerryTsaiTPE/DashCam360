@echo off
setlocal
cd /d "%~dp0"
set PYTHON_EXE=C:\Users\HPI\AppData\Local\Programs\Python\Python313\python.exe

if not exist "%PYTHON_EXE%" (
    set PYTHON_EXE=python
)

"%PYTHON_EXE%" build_portable.py
pause
