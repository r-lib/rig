@echo off
rem Managed by rig (rig proj sync). Do not edit, `rig proj sync` rewrites it.
rem The cmd.exe counterpart of `activate.bat`; cmd.exe has no functions, so
rem this has to be a file of its own.

if defined _OLD_RVENV_PATH (
    set "PATH=%_OLD_RVENV_PATH%"
    set "_OLD_RVENV_PATH="
)
if defined _OLD_RVENV_PROMPT (
    set "PROMPT=%_OLD_RVENV_PROMPT%"
    set "_OLD_RVENV_PROMPT="
)
set "RVENV="
set "R_LIBS_USER="
set "R_LIBS="
set "R_LIBS_SITE="
set "R_REPOSITORIES="
