#!/usr/bin/env bats

setup() {
    DIR="$( cd "$( dirname "$BATS_TEST_FILENAME" )" >/dev/null 2>&1 && pwd )"
    # make executables in src/ visible to PATH
    PATH="$DIR/../target/debug:$PATH"
}

teardown() {
    true
}

# These run before any `rig add`, on purpose: they must work with no R
# installed, which is when they are the most useful.

@test "system dirs" {
    run rig -q system dirs
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^Mode  *admin$"
    echo "$output" | grep -q "^Architecture  *\(arm64\|x86_64\)$"
    echo "$output" | grep -q "^R root  */Library/Frameworks/R[.]framework/Versions$"
    echo "$output" | grep -q "^Binary dir  */usr/local/bin$"
    echo "$output" | grep -q "^Config file  */"
    # TMPDIR is already per user on macOS, so only the trailing uid is fixed
    echo "$output" | grep -q "^Download dir  */.*rig-$(id -u)$"
    # rtools-dir is Windows only
    echo "$output" | grep -vq "^Rtools root"

    run rig -q system dirs --json
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q '"r_root": "/Library/Frameworks/R.framework/Versions"'
    echo "$output" | grep -q '"arch":'
    echo "$output" | grep -vq '"rtools_root"'

    run rig -q --json system dirs
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q '"binary_dir": "/usr/local/bin"'
}

@test "system dirs, single directory" {
    run rig -q system dirs --r
    [[ "$status" -eq 0 ]]
    [[ "$output" = "/Library/Frameworks/R.framework/Versions" ]]

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
    echo "$output" | grep -q "rig-$(id -u)$"
    dl="$output"
    run rig -q --user system dirs --download
    [[ "$output" = "$dl" ]]

    # both architectures share a root on macOS, so --arch is accepted and ignored
    run rig -q system dirs --r --arch x86_64
    [[ "$status" -eq 0 ]]
    [[ "$output" = "/Library/Frameworks/R.framework/Versions" ]]

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

    # the single directory and the overview agree
    run rig -q system dirs --json
    echo "$output" | grep -q "\"r_root\": \"$(rig -q system dirs --r)\""

    # the selectors are mutually exclusive and cannot be combined with --json
    run rig -q system dirs --r --binary
    [[ ! "$status" -eq 0 ]]
    run rig -q system dirs --r --json
    [[ ! "$status" -eq 0 ]]

    # hidden no-op off Windows
    run rig -q system dirs --rtools
    [[ "$status" -eq 0 ]]
    [[ -z "$output" ]]

    # hidden no-op off Linux
    run rig -q system dirs --fonts
    [[ "$status" -eq 0 ]]
    [[ -z "$output" ]]
}

@test "add" {
    if ! rig ls | grep -q '^[* ] 4.1'; then
        run sudo rig add 4.1 -a x86_64
        [[ "$status" -eq 0 ]]
        run rig ls
        echo "$output" | grep -q "^[* ] 4.1"
    fi
    run sudo rig system make-links
    [[ "$status" -eq 0 ]]
    run R-4.1 -q -s -e 'cat(as.character(getRversion()))'
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4[.]1[.][0-9]$"

    if ! rig ls | grep -q '^[* ] 4.0'; then
        run sudo rig add 4.0 -a x86_64
        [[ "$status" -eq 0 ]]
        run rig ls
        echo "$output" | grep -q "^[* ] 4.0"
    fi
    run sudo rig system make-links
    [[ "$status" -eq 0 ]]
    run R-4.0 -q -s -e 'cat(as.character(getRversion()))'
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4[.]0[.]5$"

    devel=$(rig resolve devel | cut -f1 -d" " | sed 's/\.[^..]*$//')
    if ! rig ls | grep -q "^[* ] $devel"; then
        run sudo rig add devel
        [[ "$status" -eq 0 ]]
        run rig ls
        echo "$output" | grep -q "^[* ] $devel"
    fi
    run sudo rig system make-links
    [[ "$status" -eq 0 ]]
    run R-devel -q -s -e 'cat(as.character(getRversion()))'
    [[ "$status" -eq 0 ]]
    echo $output
    echo "$output" | grep -q "^$devel[.][0-9]\$"

    if [[ "$(arch)" = "arm64" ]]; then
        if ! rig ls | grep -q '^[* ] 4.1'; then
            run sudo rig add 4.1 --arch arm64
            [[ "$status" -eq 0 ]]
            run rig ls
            echo "$output" | grep -q "^[* ] 4.1-arm64"
        fi
    fi
}

@test "default" {
    run rig default
    [[ "$status" -eq 0 ]]
    run sudo rig default 4.1
    [[ "$status" -eq 0 ]]
    run rig default
    [[ "$output" = "4.1" ]]
    run sudo rig default 1.0
    [[ ! "$status" -eq 0 ]]
    echo $output | grep -q "is not installed"
}

@test "list" {
    run rig default 4.1
    run rig list
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^[*] 4.1[ ]*[(]R 4[.]1[.][0-9][)]"
    run rig ls
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^  4.0"
}

@test "resolve" {
    run rig resolve devel
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rig resolve release
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rig resolve devel -a arm64
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rig resolve oldrel
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rig resolve -a x86_64 oldrel/3
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rig resolve 4.1.1
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "4[.]1[.]1 https://"
    run rig resolve -a x86_64 4.0
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "4[.]0[.]5 https://"
}

@test "rm" {
    if ! rig ls | grep -q '^[* ] 3.3'; then
        run sudo rig add -a x86_64 3.3 --without-pak
        [[ "$status" -eq 0 ]]
        run rig ls
        echo "$output" | grep -q "[* ] 3[.]3"
    fi
    run sudo rig rm 3.3
    [[ "$status" -eq 0 ]]
    run rig list
    echo $output | grep -vq "^[* ] 3.3$"
}

@test "system create-lib" {
    run rig system create-lib
    [[ $status -eq 0 ]]
    run R-4.1 -q -s -e 'file.exists(Sys.getenv("R_LIBS_USER"))'
    [[ $status -eq 0 ]]
    [[ "$output" = "[1] TRUE" ]]
    run R-4.0 -q -s -e 'file.exists(Sys.getenv("R_LIBS_USER"))'
    [[ $status -eq 0 ]]
    [[ "$output" = "[1] TRUE" ]]
}

@test "system add-pak" {
    run sudo rig default 4.1
    [[ "$status" -eq 0 ]]
    run rig system add-pak
    echo $output | grep -qE "(Installing|Updating) pak for R 4.1"
    run R-4.1 -q -s -e 'pak::lib_status()'
    [[ "$status" -eq 0 ]]

    if ! rig ls | grep -q '^[* ] 3.5'; then
        run sudo rig add -a x86_64 3.5
        [[ "$status" -eq 0 ]]
        run rig ls
        echo "$output" | grep -q "[* ] 3[.]5"
    fi

    libdir=`R-3.5 -s -e 'cat(path.expand(Sys.getenv("R_LIBS_USER")))'`
    [[ "$libdir" == "" ]] && false
    run sudo rm -rf "$libdir"
    run sudo rig system add-pak 3.5
    [[ "$status" -eq 0 ]]
    uid=`stat -f "%u" "$libdir"`
    [[ "$uid" -eq "`id -u`" ]]
}

@test "system fix-permissions" {
    run sudo rig system fix-permissions
    [[ "$status" -eq 0 ]]
    run ls -ld /Library/Frameworks/R.framework/Versions/4.1/Resources/library
    [[ "$status" -eq 0 ]]
    echo $output | grep -q -- "drwxr-xr-x"
}


@test "system forget" {
    run sudo rig system forget
    [[ $status -eq 0 ]]
    function pkgs {
        pkgutil --pkgs | grep -i r-project | grep -v clang
    }
    run pkgs
    [[ $status -eq 1 ]]
    [[ "$output" = "" ]]
}

@test "system make-orthogonal" {
    run sudo rig system make-orthogonal
    [[ $status -eq 0 ]]
}

@test "system no-openmp" {
    run sudo rig system no-openmp
    [[ $status -eq 0 ]]
    run grep -q fopenmp /Library/Frameworks/R.framework/Versions/4.1/Resources/etc/Makeconf
    [[ $status -eq 1 ]]
}

@test "system allow-debugger" {
    run sudo rig default 4.1
    [[ "$status" -eq 0 ]]
    run sudo rig system allow-debugger
    if [[ "$(uname -r | cut -d. -f1)" -lt "21" ]]; then
	run codesign -d --entitlements :- /Library/Frameworks/R.framework/Versions/4.1/Resources/bin/exec/R
    else
	run codesign -d --entitlements :- /Library/Frameworks/R.framework/Versions/4.1/Resources/bin/exec/R
    fi
    echo $output | grep -q -- "com.apple.security.get-task-allow"
}

@test "sysreqs" {
    run rig sysreqs list
    [[ "$status" -eq 0 ]]
    run rig sysreqs add checkbashisms tidy-html5 pkgconfig
    echo "$output"
    [[ "$status" -eq 0 ]]
    run sudo `which rig` sysreqs add checkbashisms tidy-html5 pkgconfig
    echo "$output"
    [[ "$status" -eq 0 ]]

    run rig sysreqs add checkbashisms tidy-html5 pkgconfig
    echo "$output"
    [[ "$status" -eq 0 ]]
}
