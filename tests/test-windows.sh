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
    rim.exe ls
    [[ "$status" -eq 0 ]]
    # no default initially
    if [[ ! -e "/mnt/c/Program Files/R/bin/RS.bat" ]]; then
	run rim.exe default
	[[ ! "$status" -eq 0 ]]
    fi
}

@test "add" {
    if ! cmd.exe /c rim ls | grep -q '^4.1.2$'; then
	run cmd.exe /c rim add 4.1.2
	[[ "$status" -eq 0 ]]
	run cmd.exe /c rim ls
	echo "$output" | grep -q "^4.1.2$"
    fi
    run cmd.exe /c "R-4.1.2.bat -q -s -e cat(as.character(getRversion()))"
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4[.]1[.]2$"

    if ! rim.exe ls | grep -q '^4.0.5$'; then
	run rim.exe add 4.0
	[[ "$status" -eq 0 ]]
	run rim.exe ls
	echo "$output" | grep -q "^4.0.5$"
    fi
    run cmd.exe /c "R-4.0.5.bat -q -s -e cat(as.character(getRversion()))"
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4[.]0[.]5$"

    devel=$(rim.exe resolve devel | cut -f1 -d" ")
    if ! rim.exe ls | grep -q '^devel$'; then
	run rim.exe add devel
	[[ "$status" -eq 0 ]]
	run rim.exe ls
	echo "$output" | grep -q "^devel$"
    fi
    run cmd.exe /c "R-devel.bat -q -s -e cat(as.character(getRversion()))"
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
