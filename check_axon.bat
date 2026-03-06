@echo off
cls
echo =======================================================
echo    AXON ARCHITECTURAL COPILOT - Windows Dashboard
echo =======================================================
echo.

echo [1/3] Checking Axon Daemon...
wsl uv run --project /home/dstadel/projects/axon axon daemon status
if %ERRORLEVEL% NEQ 0 (
    echo [!] Daemon is DOWN. Attempting restart...
    wsl uv run --project /home/dstadel/projects/axon axon daemon start
)
echo.

echo [2/3] Active Project Watchers (Auto-Update):
wsl ps aux | grep "axon serve --watch" | grep -v grep
if %ERRORLEVEL% NEQ 0 (
    echo [i] No active background watchers. 
    echo     Watchers start automatically when you enter a project folder in WSL.
)
echo.

echo [3/3] Last 5 Activity Events:
wsl tail -n 5 ~/.axon/events.jsonl
echo.

echo =======================================================
echo  To start a watcher manually for a project:
echo  wsl axon watch /home/dstadel/projects/PROJECT_NAME
echo =======================================================
echo.
pause
