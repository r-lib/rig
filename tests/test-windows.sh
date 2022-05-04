#!/usr/bin/env bats

setup() {
    DIR="$( cd "$( dirname "$BATS_TEST_FILENAME" )" >/dev/null 2>&1 && pwd )"
    # make executables in src/ visible to PATH
    PATH="$DIR/../target/debug:$PATH"
}

teardown() {
    true
}

# Need to test for both path forms, one from within bash, the other
# from a PowerShell Windows Terminal.

@test "empty" {
    run rim ls
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    # no default initially
    if [[ ! -e "/mnt/c/Program Files/R/bin/RS.bat" &&
	  ! -e "C:/Program Files/R/bin/RS.bat" ]]; then
	run rim default
	echo "status = ${status}"
	echo "output = ${output}"
	[[ ! "$status" -eq 0 ]]
    fi
}

# We use 4.1.1 because currently 4.1.2 is already installed on the GHA
# VM, but without the rim goodies.

@test "add" {
    if ! rim ls | grep -q '^4.1.1$'; then
	run rim add 4.1.1
	echo "status = ${status}"
	echo "output = ${output}"
	[[ "$status" -eq 0 ]]
	run rim ls
	echo "$output" | grep -q "^4.1.1$"
    fi
    run R-4.1.1.bat -q -s -e 'cat(as.character(getRversion()))'
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4[.]1[.]1$"

    if ! rim ls | grep -q '^4.0.5$'; then
	run rim add 4.0
	echo "status = ${status}"
	echo "output = ${output}"
	[[ "$status" -eq 0 ]]
	run rim ls
	echo "$output" | grep -q "^4.0.5$"
    fi
    run R-4.0.5.bat -q -s -e 'cat(as.character(getRversion()))'
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4[.]0[.]5$"

    devel=$(rim resolve devel | cut -f1 -d" ")
    if ! rim ls | grep -q '^devel$'; then
	run rim add devel
	echo "status = ${status}"
	echo "output = ${output}"
	[[ "$status" -eq 0 ]]
	run rim ls
	echo "$output" | grep -q "^devel$"
    fi
    run R-devel.bat -q -s -e 'cat(as.character(getRversion()))'
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^$devel$"
}

@test "default" {
    # no default initially
    if [[ ! -e "/mnt/c/Program Files/R/bin/RS.bat" &&
	  ! -e "C:/Program Files/R/bin/RS.bat" ]]; then
	run rim default
	echo "status = ${status}"
	echo "output = ${output}"
	[[ ! "$status" -eq 0 ]]
    fi
    run rim default 4.1.1
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    run rim default
    [[ "$output" = "4.1.1" ]]
    run rim default 1.0
    echo "status = ${status}"
    echo "output = ${output}"
    [[ ! "$status" -eq 0 ]]
    echo $output | grep -q "is not installed"
}

@test "list" {
    run rim list
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4.1.1 [(]default[)]$"
    run rim ls
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4.0.5$"
}

@test "resolve" {
    run rim resolve devel
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rim resolve release
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rim resolve oldrel
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rim resolve oldrel/3
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rim resolve 4.1.1
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "4[.]1[.]1 https://"
    run rim resolve 4.0
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "4[.]0[.]5 https://"
}

@test "rm" {
    if ! rim ls | grep -q '^3.3.3$'; then
        run rim add 3.3
	echo "status = ${status}"
	echo "output = ${output}"
        [[ "$status" -eq 0 ]]
        run rim ls
        echo "$output" | grep -q "^3[.]3[.]3$"
    fi
    run rim rm 3.3.3
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    run rim list
    echo $output | grep -vq "^3.3.3$"
}

# The quoting is very tricky here. We avoid double quotes because they
# change the Windows parsing rules.

# For the output we take the last line, in case there are warnings at
# startup. (This does happen in bash for R 4.1.1.)

@test "system create-lib" {
    # Must already exist
    run R-4.1.1.bat -q -s -e suppressWarnings\(file.exists\(Sys.getenv\(\'R_LIBS_USER\'\)\)\)
    echo "status = ${status}"
    echo "output = ${output}"
    [[ $status -eq 0 ]]
    [[ "${lines[-1]}" = "[1] TRUE" ]]
    run R-devel.bat -q -s -e file.exists\(Sys.getenv\(\'R_LIBS_USER\'\)\)
    echo "status = ${status}"
    echo "output = ${output}"
    [[ $status -eq 0 ]]
    [[ "${lines[-1]}" = "[1] TRUE" ]]
    run R-4.0.5.bat -q -s -e file.exists\(Sys.getenv\(\'R_LIBS_USER\'\)\)
    echo "status = ${status}"
    echo "output = ${output}"
    [[ $status -eq 0 ]]
    [[ "${lines[-1]}" = "[1] TRUE" ]]
    run rim system create-lib
    echo "status = ${status}"
    echo "output = ${output}"
    [[ $status -eq 0 ]]
}

@test "system add-pak" {
    run rim default 4.1.1
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    run rim system add-pak
    echo $output | grep -q "Installing pak for R 4.1.1"
    run R-4.1.1.bat -q -s -e 'pak::lib_status()'
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
}

@test "system clean-registry" {
    run rim system clean-registry
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
}

# This is tested implicitly

@test "system make-links" {
    true
}
