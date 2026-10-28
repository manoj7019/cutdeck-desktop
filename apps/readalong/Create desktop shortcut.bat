@echo off
cd /d "%~dp0"
if not exist ".venv\Scripts\pythonw.exe" (
  echo Run Readalong.bat once first, then run this again.
  pause
  exit /b 1
)
powershell -NoProfile -ExecutionPolicy ByPass -File "%~dp0make-shortcut.ps1"
echo Done. Readalong is on your desktop. You can also drag a PDF onto the icon to open it.
pause
