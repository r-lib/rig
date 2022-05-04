#!/usr/bin/env bats

setup() {
    DIR="$( cd "$( dirname "$BATS_TEST_FILENAME" )" >/dev/null 2>&1 && pwd )"
    # make executables in src/ visible to PATH
    PATH="$DIR/../target/debug:$PATH"
}

teardown() {
    true
}

@test "empty" {
    run rim ls
    [[ "$status" -eq 0 ]]
}

@test "add" {
    if ! rim ls | grep -q '^4.1.2$'; then
	run rim add 4.1.2
	[[ "$status" -eq 0 ]]
	run rim ls
	echo "$output" | grep -q "^4.1.2$"
    fi
    run R-4.1.2 -q -s -e 'cat(as.character(getRversion()))'
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4[.]1[.]2$"

    if ! rim ls | grep -q '^4.0.5$'; then
	run rim add 4.0
	[[ "$status" -eq 0 ]]
	run rim ls
	echo "$output" | grep -q "^4.0.5$"
    fi
    run R-4.0.5 -q -s -e 'cat(as.character(getRversion()))'
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4[.]0[.]5$"

    devel=$(rim resolve devel | cut -f1 -d" ")
    if ! rim ls | grep -q '^devel$'; then
	run rim add devel
	[[ "$status" -eq 0 ]]
	run rim ls
	echo "$output" | grep -q "^devel$"
    fi
    run R-devel -q -s -e 'cat(as.character(getRversion()))'
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^$devel$"
}

@test "default" {
    # no default initially
    if [[ ! -e /opt/R/current ]]; then
	run rim default
	[[ ! "$status" -eq 0 ]]
    fi
    run rim default 4.1.2
    [[ "$status" -eq 0 ]]
    run rim default
    [[ "$output" = "4.1.2" ]]
    run rim default 1.0
    [[ ! "$status" -eq 0 ]]
    echo $output | grep -q "is not installed"
}

@test "list" {
    run rim list
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4.1.2 [(]default[)]"
    run rim ls
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4.0.5$"
}

@test "resolve" {
    run rim resolve devel
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rim resolve release
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
    if ! rim ls | grep -q '^3.3.3$'; then
        run rim add 3.3
        [[ "$status" -eq 0 ]]
        run rim ls
        echo "$output" | grep -q "^3[.]3[.]3$"
    fi
    run rim rm 3.3.3
    [[ "$status" -eq 0 ]]
    run rim list
    echo $output | grep -vq "^3.3.3$"
}

@test "system create-lib" {
    # Must already exist
    run R-4.1.2 -q -s -e 'file.exists(Sys.getenv("R_LIBS_USER"))'
    [[ $status -eq 0 ]]
    [[ "$output" = "[1] TRUE" ]]
    run R-devel -q -s -e 'file.exists(Sys.getenv("R_LIBS_USER"))'
    [[ $status -eq 0 ]]
    [[ "$output" = "[1] TRUE" ]]
    run R-4.0.5 -q -s -e 'file.exists(Sys.getenv("R_LIBS_USER"))'
    [[ $status -eq 0 ]]
    [[ "$output" = "[1] TRUE" ]]
    run rim system create-lib
    [[ $status -eq 0 ]]
}

@test "system add-pak" {
    run rim default 4.1.2
    [[ "$status" -eq 0 ]]
    run rim system add-pak
    echo $output | grep -q "Installing pak for R 4.1.2"
    run R-4.1.2 -q -s -e 'pak::lib_status()'
    [[ "$status" -eq 0 ]]
}
