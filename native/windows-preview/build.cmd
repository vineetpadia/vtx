@echo off
setlocal
for /f "usebackq tokens=*" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -products * -latest -property installationPath`) do set "VTX_VS=%%i"
if not defined VTX_VS exit /b 1
call "%VTX_VS%\VC\Auxiliary\Build\vcvars64.bat" >nul
if not exist "%~dp0build" mkdir "%~dp0build"
if not defined CARGO_HOME set "CARGO_HOME=%USERPROFILE%\.cargo"
for /r "%CARGO_HOME%\registry\src" %%i in (libdeflate.h) do if exist "%%i" set "VTX_DEFLATE_INCLUDE=%%~dpi"
for /r "%~dp0..\target\release\build" %%i in (deflate.lib) do if exist "%%i" set "VTX_DEFLATE_LIB=%%i"
if not defined VTX_DEFLATE_INCLUDE (
    echo Build the Rust release example first to fetch libdeflate.
    exit /b 1
)
if not defined VTX_DEFLATE_LIB (
    echo Build the Rust release example first to compile libdeflate.
    exit /b 1
)
cl /nologo /std:c++17 /O2 /EHsc /MT /DUNICODE /D_UNICODE /I"%VTX_DEFLATE_INCLUDE%." /LD "%~dp0preview.cpp" /Fo"%~dp0build\preview.obj" /Fe"%~dp0build\VTXPreviewHD.dll" /link /DEF:"%~dp0preview.def" "%VTX_DEFLATE_LIB%" ole32.lib windowscodecs.lib shell32.lib user32.lib gdi32.lib msimg32.lib uuid.lib
if errorlevel 1 exit /b %errorlevel%
cl /nologo /std:c++17 /O2 /EHsc /MT /DUNICODE /D_UNICODE "%~dp0preview_test.cpp" /Fo"%~dp0build\preview_test.obj" /Fe"%~dp0build\preview_test.exe" /link ole32.lib user32.lib uuid.lib
if errorlevel 1 exit /b %errorlevel%
cl /nologo /std:c++17 /O2 /EHsc /MT /I"%VTX_DEFLATE_INCLUDE%." "%~dp0saved_preview_test.cpp" /Fo"%~dp0build\saved_preview_test.obj" /Fe"%~dp0build\saved_preview_test.exe" /link "%VTX_DEFLATE_LIB%"
if errorlevel 1 exit /b %errorlevel%
"%~dp0build\saved_preview_test.exe"
if errorlevel 1 exit /b %errorlevel%
copy /y "%~dp0..\..\assets\vtx.ico" "%~dp0build\VTX.ico" >nul
exit /b %errorlevel%
