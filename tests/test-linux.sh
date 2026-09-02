#!/usr/bin/env bats

setup() {
    DIR="$( cd "$( dirname "$BATS_TEST_FILENAME" )" >/dev/null 2>&1 && pwd )"
    # make executables in src/ visible to PATH
    SUDO="$(if [ "$EUID" -ne 0 ]; then echo sudo; else echo ''; fi)"
}

teardown() {
    true
}

@test "empty" {
    run rig ls
    [[ "$status" -eq 0 ]]
}

# These run before any `rig add`, on purpose: they must work with no R
# installed, which is when they are the most useful.

@test "system dirs" {
    run rig -q system dirs
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^Mode  *admin$"
    echo "$output" | grep -q "^Architecture  *[a-z0-9_]"
    echo "$output" | grep -q "^R root  */opt/R$"
    echo "$output" | grep -q "^Binary dir  */usr/local/bin$"
    echo "$output" | grep -q "^Config file  */"
    # the fontconfig dir is Linux only
    echo "$output" | grep -q "^Fonts dir  */opt/rig/fontconfig$"
    echo "$output" | grep -q "^Download dir  */tmp/rig-$(id -u)$"
    # rtools-dir is Windows only
    echo "$output" | grep -vq "^Rtools root"

    run rig -q system dirs --json
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q '"r_root": "/opt/R"'
    echo "$output" | grep -q '"arch":'
    echo "$output" | grep -q '"fonts_dir": "/opt/rig/fontconfig"'
    echo "$output" | grep -q "\"download_dir\": \"/tmp/rig-$(id -u)\""
    echo "$output" | grep -vq '"rtools_root"'

    run rig -q --json system dirs
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q '"binary_dir": "/usr/local/bin"'
}

@test "system dirs, single directory" {
    run rig -q system dirs --r
    [[ "$status" -eq 0 ]]
    [[ "$output" = "/opt/R" ]]

    run rig -q system dirs --binary
    [[ "$status" -eq 0 ]]
    [[ "$output" = "/usr/local/bin" ]]

    for opt in --data --cache --download --log; do
	run rig -q system dirs $opt
	[[ "$status" -eq 0 ]]
	echo "$output" | grep -q "^/"
    done

    # the download directory is per user id, and not per mode: rig escalates
    # before it downloads in admin mode, so the uid already tells the two apart
    run rig -q system dirs --download
    [[ "$output" = "/tmp/rig-$(id -u)" ]]
    run rig -q --user system dirs --download
    [[ "$output" = "/tmp/rig-$(id -u)" ]]

    # the effective value follows the overrides
    run env RIG_R_INSTALL_DIR=/tmp/rig-r rig -q system dirs --r
    [[ "$output" = "/tmp/rig-r" ]]
    run env RIG_BINARY_DIR=/tmp/rig-bin rig -q system dirs --binary
    [[ "$output" = "/tmp/rig-bin" ]]
    run env RIG_DOWNLOAD_DIR=/tmp/rig-dl rig -q system dirs --download
    [[ "$output" = "/tmp/rig-dl" ]]

    run env RIG_MODE=user rig -q system dirs --r
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "/[.]local/share/rig/r$"
    run rig -q --user system dirs --binary
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "/[.]local/bin$"

    # the fontconfig dir is outside of the R installations, in both modes:
    # in rig's own directory in admin mode, in the data directory in user mode
    run rig -q system dirs --fonts
    [[ "$status" -eq 0 ]]
    [[ "$output" = "/opt/rig/fontconfig" ]]
    run rig -q --user system dirs --fonts
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "/[.]local/share/rig/fontconfig$"
    run env RIG_R_INSTALL_DIR=/tmp/rig-r rig -q system dirs --fonts
    [[ "$output" = "/tmp/fontconfig" ]]

    # the single directory and the overview agree
    run rig -q system dirs --json
    echo "$output" | grep -q "\"r_root\": \"$(rig -q system dirs --r)\""
    echo "$output" | grep -q "\"fonts_dir\": \"$(rig -q system dirs --fonts)\""

    # the selectors are mutually exclusive and cannot be combined with --json
    run rig -q system dirs --r --binary
    [[ ! "$status" -eq 0 ]]
    run rig -q system dirs --r --json
    [[ ! "$status" -eq 0 ]]

    # hidden no-op off Windows
    run rig -q system dirs --rtools
    [[ "$status" -eq 0 ]]
    [[ -z "$output" ]]
}

@test "--no-cache" {
    # accepted before and after the subcommand, and from the environment
    run rig -q --no-cache system dirs --cache
    [[ "$status" -eq 0 ]]
    cache="$output"
    run rig -q system dirs --no-cache --cache
    [[ "$status" -eq 0 ]]
    [[ "$output" = "$cache" ]]
    run env RIG_NO_CACHE=true rig -q system dirs --cache
    [[ "$status" -eq 0 ]]
    # the reported cache directory is the real one either way: --no-cache
    # changes where this run keeps things, not where rig keeps them
    [[ "$output" = "$cache" ]]
    [[ "$output" = "$(rig -q system dirs --cache)" ]]

    # a value that is not a boolean is a warning, not a failure
    run env RIG_NO_CACHE=perhaps rig -q system dirs --cache
    [[ "$status" -eq 0 ]]
    [[ "$output" = "$cache" ]]

    # a command that never looks at the cache does not create a throwaway
    # directory for it either, and one that does cleans it up afterwards
    run bash -c 'ls -d "${TMPDIR:-/tmp}"/rig-nocache-* 2>/dev/null | wc -l'
    [[ "$output" -eq 0 ]]
}

@test "add" {
    if ! rig ls | grep -q '^[* ] 4.5.1'; then
	run rig -v add 4.5.1
	[[ "$status" -eq 0 ]]
	run rig ls
	echo "$output" | grep -q "^[* ] 4.5.1"
    fi
    run R-4.5.1 -q -s -e 'cat(as.character(getRversion()))'
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4[.]5[.]1$"

    if ! rig ls | grep -q '^[* ] 4.4.3'; then
	run rig add 4.4
	[[ "$status" -eq 0 ]]
	run rig ls
	echo "$output" | grep -q "^[* ] 4.4.3"
    fi
    run R-4.4.3 -q -s -e 'cat(as.character(getRversion()))'
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4[.]4[.]3$"

    devel=$(rig resolve devel | cut -f1 -d" ")
    if ! rig ls | grep -q '^[* ] devel$'; then
	run rig add devel
	[[ "$status" -eq 0 ]]
	run rig ls
	echo "$output" | grep -q "^[* ] devel"
    fi
    run R-devel -q -s -e 'cat(as.character(getRversion()))'
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^$devel$"
}

@test "default" {
    # no default initially
    if [[ ! -e /opt/R/current ]]; then
	run rig default
	[[ ! "$status" -eq 0 ]]
    fi
    run rig default 4.5.1
    [[ "$status" -eq 0 ]]
    run rig -q default
    [[ "$status" -eq 0 ]]
    echo "Output was:"
    echo "$output"
    [[ "$output" = "4.5.1" ]]
    run rig default 1.0
    [[ ! "$status" -eq 0 ]]
    echo $output | grep -q "is not installed"
}

@test "list" {
    run rig list
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^[*] 4.5.1"
    run rig ls
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^  4.4.3"
}

@test "resolve" {
    run rig resolve devel
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rig resolve release
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rig resolve oldrel
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rig resolve oldrel/3
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rig resolve 4.5.1
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "4[.]5[.]1 https://"
    run rig resolve 4.4
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "4[.]4[.]3 https://"
}

@test "rm" {
    if ! rig ls | grep -q '^[* ] 4.0.5$'; then
        run rig add 4.0 --without-pak
        [[ "$status" -eq 0 ]]
        run rig ls
        echo "$output" | grep -q "^[* ] 4[.]0[.]5"
    fi
    run rig rm 4.0.5
    [[ "$status" -eq 0 ]]
    run rig list
    echo $output | grep -vq "^[* ] 4.0.5"
}

@test "system create-lib" {
    # Must already exist
    run R-4.5.1 -q -s -e 'file.exists(Sys.getenv("R_LIBS_USER"))'
    [[ $status -eq 0 ]]
    [[ "$output" = "[1] TRUE" ]]
    run R-devel -q -s -e 'file.exists(Sys.getenv("R_LIBS_USER"))'
    [[ $status -eq 0 ]]
    [[ "$output" = "[1] TRUE" ]]
    run R-4.4.3 -q -s -e 'file.exists(Sys.getenv("R_LIBS_USER"))'
    [[ $status -eq 0 ]]
    [[ "$output" = "[1] TRUE" ]]
    run rig -vv system create-lib
    echo "$output"
    [[ $status -eq 0 ]]
}

@test "system add-pak" {
    if ! rig ls | grep -q '^[* ] 4.5.1'; then
	run rig -v add 4.5.1
	[[ "$status" -eq 0 ]]
    fi
    run rig default 4.5.1
    [[ "$status" -eq 0 ]]
    run rig system add-pak
    echo $output | grep -qE "(Installing|Updating) pak for R 4.5.1"
    run R-4.5.1 -q -s -e 'pak::lib_status()'
    [[ "$status" -eq 0 ]]

    if ! rig ls | grep -q '^[* ] 4.0.5$'; then
        run rig add 4.0.5
        [[ "$status" -eq 0 ]]
        run rig ls
        echo "$output" | grep -q "^[* ] 4[.]0[.]5"
    fi

    libdir=`R-4.0.5 -s -e 'cat(path.expand(Sys.getenv("R_LIBS_USER")))'`
    [[ "$libdir" == "" ]] && false
    run $SUDO rm -rf "$libdir"
    run $SUDO `which rig` system add-pak 4.0.5
    [[ "$status" -eq 0 ]]
    uid=`stat -c "%u" "$libdir"`
    [[ "$uid" -eq "`id -u`" ]]
}

@test "proj init" {
    cd "$BATS_TEST_TMPDIR"
    rm -rf myproj && mkdir myproj && cd myproj

    # No R needs to be installed for the requested version, `rig proj init`
    # does not touch an R installation.
    run rig proj init -r 4.5.1
    [[ "$status" -eq 0 ]]
    [[ -f rproj.toml ]]
    [[ -f .Renviron ]]
    [[ -f .gitignore ]]
    [[ -f .rvenv/lib/.gitignore ]]
    [[ -f .rvenv/lib/rig/DESCRIPTION ]]
    grep -q '^name = "myproj"$' rproj.toml
    grep -q '^R = ">= 4.5"$' rproj.toml
    grep -q '^R_LIBS_USER=.rvenv/lib$' .Renviron
    grep -q '^!/.rvenv/lib$' .gitignore
    grep -q '^Package: rig$' .rvenv/lib/rig/DESCRIPTION

    # The IDE leg: a plain R session in the project picks up the shim
    # package, which resolves the library path and warns about the missing
    # sync.
    run env -u RVENV R-4.5.1 -q -s -e 'cat(.libPaths()[1])'
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "Project is not synced"
    echo "$output" | grep -q "myproj/[.]rvenv/lib"

    # Refuses to overwrite, and says what is in the way
    run rig proj init -r 4.5.1
    [[ "$status" -ne 0 ]]
    echo "$output" | grep -q "rproj.toml"
    echo "$output" | grep -q -- "--force"

    # --force keeps the user's own ignore rules, rig only manages its block
    echo "*.log" >> .gitignore
    run rig proj init -r 4.5.1 --force
    [[ "$status" -eq 0 ]]
    grep -q '^[*].log$' .gitignore
    [[ "$(grep -c '^# rig rvenv start$' .gitignore)" -eq 1 ]]
}
