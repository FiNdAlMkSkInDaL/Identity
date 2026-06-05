@echo off
cd /d "%~dp0"
".\target\release\identityd.exe" start %*
