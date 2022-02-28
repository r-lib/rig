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
    # no default initially
    if [[ ! -e "/mnt/c/Program Files/R/bin/RS.bat" ]]; then
	run rim default
	[[ ! "$status" -eq 0 ]]
    fi
}

@test "add" {
    if ! rim ls | grep -q '^4.1.1$'; then
	run rim add 4.1.1
	[[ "$status" -eq 0 ]]
	run rim ls
	echo "$output" | grep -q "^4.1.1$"
    fi
    run R-4.1.1.bat -q -s -e 'cat(as.character(getRversion()))'
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4[.]1[.]2$"

    if ! rim ls | grep -q '^4.0.5$'; then
	run rim add 4.0
	[[ "$status" -eq 0 ]]
	run rim ls
	echo "$output" | grep -q "^4.0.5$"
    fi
    run R-4.0.5.bat -q -s -e 'cat(as.character(getRversion()))'
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4[.]0[.]5$"

    devel=$(rim resolve devel | cut -f1 -d" ")
    if ! rim ls | grep -q '^devel$'; then
	run rim add devel
	[[ "$status" -eq 0 ]]
	run rim ls
	echo "$output" | grep -q "^devel$"
    fi
    run R-devel.bat -q -s -e 'cat(as.character(getRversion()))'
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^$devel$"
}

@test "default" {
    true
}

@test "list" {
    true
}

@test "resolve" {
    true
}

@test "rm" {
    true
}

@test "system create-lib" {
    true
}

@test "system add-pak" {
    true
}

@test "system clean-registry" {
    true
}

@test "system make-links" {
    true
}
