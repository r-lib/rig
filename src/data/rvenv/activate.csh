# Managed by rig (rig proj sync). Do not edit, `rig proj sync` rewrites it.
#
# For csh and tcsh. Source it, do not run it:
#
#     source .rvenv/bin/activate.csh
#
# The project path below is absolute, baked in by `rig proj sync`. Re-run
# `rig proj sync` after moving the project.

alias deactivate 'test $?_OLD_RVENV_PATH != 0 && setenv PATH "$_OLD_RVENV_PATH" && unset _OLD_RVENV_PATH; test $?_OLD_RVENV_PROMPT != 0 && set prompt="$_OLD_RVENV_PROMPT" && unset _OLD_RVENV_PROMPT; unsetenv RVENV; unsetenv R_LIBS_USER; unsetenv R_LIBS; unsetenv R_LIBS_SITE; unsetenv R_REPOSITORIES; test "\!:*" != "nondestructive" && unalias deactivate; rehash'

deactivate nondestructive

setenv RVENV "@RVENV@"
set _OLD_RVENV_PATH="$PATH"
setenv PATH "$RVENV/bin:$PATH"
setenv R_LIBS_USER "$RVENV/lib"
setenv R_LIBS ""
setenv R_LIBS_SITE /nonexistent/rvenv-no-site
setenv R_REPOSITORIES "$RVENV/etc/repositories"

# `prompt` is only set in an interactive shell, hence the second test.
if (! $?RVENV_DISABLE_PROMPT && $?prompt) then
    set _OLD_RVENV_PROMPT="$prompt:q"
    set prompt = "(@RVENV_NAME@) $prompt:q"
endif

rehash
