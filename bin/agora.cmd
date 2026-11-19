@echo off
rem Windows launcher (best-effort, untested). AGORA_BIN overrides; else downloads the pinned release.
setlocal
if defined AGORA_BIN if exist "%AGORA_BIN%" ( "%AGORA_BIN%" %* & exit /b %errorlevel% )
set "ROOT=%CLAUDE_PLUGIN_ROOT%"
if not defined ROOT set "ROOT=%~dp0.."
set "DATA=%CLAUDE_PLUGIN_DATA%"
if not defined DATA set "DATA=%LOCALAPPDATA%\agora-mesh-plugin"
set "VERSION="
for /f "usebackq delims=" %%v in (`powershell -NoProfile -Command "(Get-Content -Raw '%ROOT%\.claude-plugin\plugin.json' | ConvertFrom-Json).version"`) do set "VERSION=%%v"
if not defined VERSION ( echo agora: cannot read version from plugin.json 1>&2 & exit /b 1 )
set "BASE=%AGORA_RELEASE_BASE%"
if not defined BASE set "BASE=https://github.com/kn0sti/agora-mesh/releases/download"
set "BASE=%BASE%/v%VERSION%"
set "ASSET=agora-x86_64-pc-windows-msvc.exe"
set "DIR=%DATA%\bin\%VERSION%"
set "BIN=%DIR%\agora.exe"
if exist "%BIN%" goto run
if not exist "%DIR%" mkdir "%DIR%"
echo agora: downloading %ASSET% v%VERSION% ... 1>&2
powershell -NoProfile -Command "$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'; $t='%BIN%.part'; Invoke-WebRequest '%BASE%/%ASSET%' -OutFile $t; $s=(Invoke-WebRequest '%BASE%/SHA256SUMS').Content; $w=($s -split \"`n\" | ForEach-Object { $p=$_.Trim() -split '\s+'; if($p.Count -ge 2 -and $p[1].TrimStart('*') -eq '%ASSET%'){$p[0]} } | Select-Object -First 1); if(-not $w){Remove-Item $t; Write-Error 'not listed in SHA256SUMS'}; if((Get-FileHash $t -Algorithm SHA256).Hash.ToLower() -ne $w.ToLower()){Remove-Item $t; Write-Error 'checksum mismatch'}; Move-Item -Force $t '%BIN%'"
if errorlevel 1 ( echo agora: download or verification failed 1>&2 & exit /b 1 )
:run
"%BIN%" %*
exit /b %errorlevel%
