@echo off
setlocal
cd /d "%~dp0"
set "PATH=%USERPROFILE%\.local\bin;%PATH%"

where uv >nul 2>nul
if errorlevel 1 (
  echo Installing uv, a small tool that sets up Python for Readalong. This happens once.
  powershell -NoProfile -ExecutionPolicy ByPass -Command "irm https://astral.sh/uv/install.ps1 | iex"
  set "PATH=%USERPROFILE%\.local\bin;%PATH%"
)

if not exist ".venv\Scripts\pythonw.exe" (
  echo.
  echo Setting up Readalong for the first time. This takes a minute or two...
  echo.
)
uv sync --python 3.12 --quiet
if errorlevel 1 (
  echo.
  echo Setup didn't finish. Check your internet connection and run Readalong.bat again.
  pause
  exit /b 1
)

start "" ".venv\Scripts\pythonw.exe" "%~dp0readalong.py" %*
