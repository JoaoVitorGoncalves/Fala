@echo off
rem ============================================================
rem Build do Fala no Windows com aceleração CUDA (GPU NVIDIA).
rem Requisitos: CUDA Toolkit (CUDA_PATH apontando para a raiz),
rem ninja no PATH (.cargo\bin) e o whisper-rs com feature "cuda".
rem Uso: scripts\build-windows-cuda.cmd  (roda pnpm tauri build)
rem ============================================================
set "PATH=C:\Users\senho\.cargo\bin;%PATH%"
if not defined CUDA_PATH set "CUDA_PATH=C:\Users\senho\cuda-noadmin\v12.9"
set "PATH=%CUDA_PATH%\bin;%PATH%"
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
cd /d "%~dp0.."
set "CMAKE_GENERATOR=Ninja"
pnpm tauri build
if exist "%CUDA_PATH%\bin\cublas64_12.dll" (
    copy /y "%CUDA_PATH%\bin\*.dll" "%~dp0..\src-tauri\target\release\" >nul
)
if exist "%~dp0sign-windows.ps1" (
    powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0sign-windows.ps1"
)
