# Managed by rig (rig proj sync). Do not edit, `rig proj sync` rewrites it.
#
# For fish. Source it, do not run it:
#
#     source .rvenv/bin/activate.fish
#
# The project path below is absolute, baked in by `rig proj sync`. Re-run
# `rig proj sync` after moving the project.

function deactivate -d "Leave the rig project environment"
    if test -n "$_OLD_RVENV_PATH"
        set -gx PATH $_OLD_RVENV_PATH
        set -e _OLD_RVENV_PATH
    end
    if functions -q _old_fish_prompt
        functions -e fish_prompt
        functions -c _old_fish_prompt fish_prompt
        functions -e _old_fish_prompt
    end
    set -e RVENV
    set -e R_LIBS_USER
    set -e R_LIBS
    set -e R_LIBS_SITE
    set -e R_REPOSITORIES
    if test "$argv[1]" != "nondestructive"
        functions -e deactivate
    end
end

deactivate nondestructive

set -gx RVENV "@RVENV@"
set -g _OLD_RVENV_PATH $PATH
set -gx PATH "$RVENV/bin" $PATH
set -gx R_LIBS_USER "$RVENV/lib"
set -gx R_LIBS ""
set -gx R_LIBS_SITE /nonexistent/rvenv-no-site
set -gx R_REPOSITORIES "$RVENV/etc/repositories"

if test -z "$RVENV_DISABLE_PROMPT"
    functions -c fish_prompt _old_fish_prompt
    function fish_prompt
        printf "%s(%s)%s " (set_color normal) "@RVENV_NAME@" (set_color normal)
        _old_fish_prompt
    end
end
