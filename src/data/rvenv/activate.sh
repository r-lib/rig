# Managed by rig (rig proj sync). Do not edit, `rig proj sync` rewrites it.
#
# This file must be sourced, not run:
#
#     . .rvenv/bin/activate
#
# A sourced script cannot find its own path portably, so the project path
# below is absolute, baked in by `rig proj sync`. Re-run `rig proj sync`
# after moving the project.
#
# You do not need this file to use the project: `.rvenv/bin/R` and
# `.rvenv/bin/Rscript` set the same environment on their own, and R started
# by an IDE picks the project up through `.Renviron`.

deactivate() {
    if [ -n "${_OLD_RVENV_PATH:-}" ] || [ "${_OLD_RVENV_PATH-x}" != x ]; then
        PATH="$_OLD_RVENV_PATH"
        export PATH
        unset _OLD_RVENV_PATH
    fi
    if [ -n "${_OLD_RVENV_PS1:-}" ] || [ "${_OLD_RVENV_PS1-x}" != x ]; then
        PS1="$_OLD_RVENV_PS1"
        export PS1
        unset _OLD_RVENV_PS1
    fi
    unset RVENV
    unset R_LIBS_USER
    unset R_LIBS
    unset R_LIBS_SITE
    unset R_REPOSITORIES
    if [ ! "${1:-}" = "nondestructive" ]; then
        unset -f deactivate
    fi
    # Forget the hashed locations of R and Rscript.
    if [ -n "${BASH:-}" ] || [ -n "${ZSH_VERSION:-}" ]; then
        hash -r 2>/dev/null
    fi
}

# Start from a clean slate, in case another environment is active.
deactivate nondestructive

RVENV="@RVENV@"
export RVENV
_OLD_RVENV_PATH="$PATH"
PATH="$RVENV/bin:$PATH"
export PATH
export R_LIBS_USER="$RVENV/lib"
export R_LIBS=
export R_LIBS_SITE=/nonexistent/rvenv-no-site
export R_REPOSITORIES="$RVENV/etc/repositories"

if [ -z "${RVENV_DISABLE_PROMPT:-}" ]; then
    _OLD_RVENV_PS1="${PS1:-}"
    PS1="(@RVENV_NAME@) ${PS1:-}"
    export PS1
fi

if [ -n "${BASH:-}" ] || [ -n "${ZSH_VERSION:-}" ]; then
    hash -r 2>/dev/null
fi
