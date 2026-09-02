@echo off
rem Managed by rig (rig proj sync). Do not edit, `rig proj sync` rewrites it.
rem
rem Run it from cmd.exe:
rem
rem     .rvenv\bin\activate.bat
rem
rem The project path below is absolute, baked in by `rig proj sync`. Re-run
rem `rig proj sync` after moving the project.

set "RVENV=@RVENV@"

if defined _OLD_RVENV_PATH (
    set "PATH=%_OLD_RVENV_PATH%"
) else (
    set "_OLD_RVENV_PATH=%PATH%"
)
set "PATH=%RVENV%\bin;%PATH%"

if defined _OLD_RVENV_PROMPT (
    set "PROMPT=%_OLD_RVENV_PROMPT%"
) else (
    set "_OLD_RVENV_PROMPT=%PROMPT%"
)
if not defined RVENV_DISABLE_PROMPT set "PROMPT=(@RVENV_NAME@) %PROMPT%"

set "R_LIBS_USER=%RVENV%\lib"
set "R_LIBS="
set "R_LIBS_SITE=/nonexistent/rvenv-no-site"
set "R_REPOSITORIES=%RVENV%\etc\repositories"
