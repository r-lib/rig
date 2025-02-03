#!/usr/bin/env bats

setup() {
    DIR="$( cd "$( dirname "$BATS_TEST_FILENAME" )" >/dev/null 2>&1 && pwd )"
    # make executables in src/ visible to PATH
    PATH="$DIR/../target/debug:$PATH"
}

teardown() {
    true
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
    echo $output | grep -q "Installing pak for R 4.1"
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
    run ls -l /Library/Frameworks/R.framework/Versions/4.1/Resources/Rscript
    [[ "$status" -eq 0 ]]
    echo $output | grep -q -- "-rwxr-xr-x"
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
