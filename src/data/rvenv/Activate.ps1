# Managed by rig (rig proj sync). Do not edit, `rig proj sync` rewrites it.
#
# Dot-source it from PowerShell:
#
#     . .rvenv\bin\Activate.ps1
#
# The project path below is absolute, baked in by `rig proj sync`. Re-run
# `rig proj sync` after moving the project.

function global:deactivate([switch]$NonDestructive) {
    if (Test-Path -Path Function:_old_virtual_prompt) {
        Copy-Item -Path Function:_old_virtual_prompt -Destination Function:prompt
        Remove-Item -Path Function:_old_virtual_prompt
    }
    if (Test-Path -Path env:_OLD_RVENV_PATH) {
        Copy-Item -Path env:_OLD_RVENV_PATH -Destination env:PATH
        Remove-Item -Path env:_OLD_RVENV_PATH
    }
    foreach ($name in "RVENV", "R_LIBS_USER", "R_LIBS", "R_LIBS_SITE", "R_REPOSITORIES") {
        if (Test-Path -Path "env:$name") {
            Remove-Item -Path "env:$name"
        }
    }
    if (-not $NonDestructive) {
        Remove-Item -Path Function:deactivate
    }
}

deactivate -NonDestructive

$env:RVENV = "@RVENV@"
Copy-Item -Path env:PATH -Destination env:_OLD_RVENV_PATH
$env:PATH = "$env:RVENV\bin;$env:PATH"
$env:R_LIBS_USER = "$env:RVENV\lib"
$env:R_LIBS = ""
$env:R_LIBS_SITE = "/nonexistent/rvenv-no-site"
$env:R_REPOSITORIES = "$env:RVENV\etc\repositories"

if (-not $env:RVENV_DISABLE_PROMPT) {
    Copy-Item -Path Function:prompt -Destination Function:_old_virtual_prompt
    function global:prompt {
        Write-Host -NoNewline -ForegroundColor Green "(@RVENV_NAME@) "
        _old_virtual_prompt
    }
}
