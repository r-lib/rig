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
    if ! rim ls | grep -q '^4.1$'; then
        run sudo rim add 4.1
        [[ "$status" -eq 0 ]]
        run rim ls
        echo "$output" | grep -q "^4.1$"
    fi
    run R-4.1 -q -s -e 'cat(as.character(getRversion()))'
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4[.]1[.][0-9]$"

    if ! rim ls | grep -q '^4.0$'; then
        run sudo rim add 4.0
        [[ "$status" -eq 0 ]]
        run rim ls
        echo "$output" | grep -q "^4.0$"
    fi
    run R-4.0 -q -s -e 'cat(as.character(getRversion()))'
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4[.]0[.]5$"

    devel=$(rim resolve devel | cut -f1 -d" " | sed 's/\.[^..]*$//')
    if ! rim ls | grep -q "^$devel\$"; then
        run sudo rim add devel
        [[ "$status" -eq 0 ]]
        run rim ls
        echo "$output" | grep -q "^$devel\$"
    fi
    run R-${devel} -q -s -e 'cat(as.character(getRversion()))'
    [[ "$status" -eq 0 ]]
    echo $output
    echo "$output" | grep -q "^$devel[.][0-9]\$"

    if [[ "$(arch)" = "arm64" ]]; then
        if ! rim ls | grep -q '^4.1$'; then
            run sudo rim add 4.1 --arch arm64
            [[ "$status" -eq 0 ]]
            run rim ls
            echo "$output" | grep -q "^4.1-arm64$"
        fi
    fi
}

@test "default" {
    run rim default
    [[ "$status" -eq 0 ]]
    run sudo rim default 4.1
    [[ "$status" -eq 0 ]]
    run rim default
    [[ "$output" = "4.1" ]]
    run sudo rim default 1.0
    [[ ! "$status" -eq 0 ]]
    echo $output | grep -q "is not installed"
}

@test "list" {
    run rim list
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4.1$"
    run rim ls
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4.0$"
}

@test "resolve" {
    run rim resolve devel
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rim resolve release
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rim resolve devel -a arm64
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rim resolve oldrel
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rim resolve oldrel/3
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rim resolve 4.1.1
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "4[.]1[.]1 https://"
    run rim resolve 4.0
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "4[.]0[.]5 https://"
}

@test "rm" {
    if ! rim ls | grep -q '^3.3$'; then
        run sudo rim add 3.3
        [[ "$status" -eq 0 ]]
        run rim ls
        echo "$output" | grep -q "^3[.]3$"
    fi
    run sudo rim rm 3.3
    [[ "$status" -eq 0 ]]
    run rim list
    echo $output | grep -vq "^3.3$"
}

@test "system create-lib" {
    run rim system create-lib
    [[ $status -eq 0 ]]
    run R-4.1 -q -s -e 'file.exists(Sys.getenv("R_LIBS_USER"))'
    [[ $status -eq 0 ]]
    [[ "$output" = "[1] TRUE" ]]
    run R-4.0 -q -s -e 'file.exists(Sys.getenv("R_LIBS_USER"))'
    [[ $status -eq 0 ]]
    [[ "$output" = "[1] TRUE" ]]
}

@test "system add-pak" {
    run sudo rim default 4.1
    [[ "$status" -eq 0 ]]
    run rim system add-pak
    echo $output | grep -q "Installing pak for R 4.1"
    run R-4.1 -q -s -e 'pak::lib_status()'
    [[ "$status" -eq 0 ]]
}

@test "system fix-permissions" {
    run sudo rim system fix-permissions
    [[ "$status" -eq 0 ]]
    run ls -l /Library/Frameworks/R.framework/Versions/4.1/Resources/Rscript
    [[ "$status" -eq 0 ]]
    echo $output | grep -q -- "-rwxr-xr-x"
}


@test "system forget" {
    run sudo rim system forget
    [[ $status -eq 0 ]]
    function pkgs {
        pkgutil --pkgs | grep -i r-project | grep -v clang
    }
    run pkgs
    [[ $status -eq 0 ]]
    [[ "$output" = "" ]]
}

@test "system make-orthogonal" {
    run sudo rim system make-orthogonal
    [[ $status -eq 0 ]]
}

@test "system no-openmp" {
    run sudo rim system no-openmp
    [[ $status -eq 0 ]]
    run grep -q fopenmp /Library/Frameworks/R.framework/Versions/4.1/Resources/etc/Makeconf
    [[ $status -eq 1 ]]
}

@test "system allow-debugger" {
    run sudo rim default 4.1
    [[ "$status" -eq 0 ]]
    run sudo rim system allow-debugger
    if [[ "$(uname -r | cut -d. -f1)" -lt "21" ]]; then
	run codesign -d --entitlements :- /Library/Frameworks/R.framework/Versions/4.1/Resources/bin/exec/R
    else
	run codesign -d --entitlements :- /Library/Frameworks/R.framework/Versions/4.1/Resources/bin/exec/R
    fi
    echo $output | grep -q -- "com.apple.security.get-task-allow"
}
