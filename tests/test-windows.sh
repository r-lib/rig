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
    run rig ls
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    # no default initially
    if [[ ! -e "/mnt/c/Program Files/R/bin/RS.bat" &&
	  ! -e "C:/Program Files/R/bin/RS.bat" ]]; then
	run rig default
	echo "status = ${status}"
	echo "output = ${output}"
	[[ ! "$status" -eq 0 ]]
    fi
}

# We use 4.5.0 because currently 4.5.1 is already installed on the GHA
# VM, but without the rig goodies.

@test "add" {
    if ! rig ls | grep -q '^[* ] 4.5.0$'; then
	run rig add 4.5.0
	echo "status = ${status}"
	echo "output = ${output}"
	[[ "$status" -eq 0 ]]
	run rig ls
	echo "$output" | grep -q "^[* ] 4.5.0"
    fi
    run R-4.5.0.bat -q -s -e 'cat(as.character(getRversion()))'
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4[.]5[.]0$"

    if ! rig ls | grep -q '^[* ] 4.4.3$'; then
	run rig add 4.4
	echo "status = ${status}"
	echo "output = ${output}"
	[[ "$status" -eq 0 ]]
	run rig ls
	echo "$output" | grep -q "^[* ] 4.4.3"
    fi
    run R-4.4.3.bat -q -s -e 'cat(as.character(getRversion()))'
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4[.]4[.]3$"

    devel=$(rig resolve devel | cut -f1 -d" ")
    if ! rig ls | grep -q '^[* ] devel$'; then
	run rig add devel
	echo "status = ${status}"
	echo "output = ${output}"
	[[ "$status" -eq 0 ]]
	run rig ls
	echo "$output" | grep -q "^[* ] devel"
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
	run rig default
	echo "status = ${status}"
	echo "output = ${output}"
	[[ ! "$status" -eq 0 ]]
    fi
    run rig default 4.5.0
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    run rig -q default
    [[ "$output" = "4.5.0" ]]
    run rig default 1.0
    echo "status = ${status}"
    echo "output = ${output}"
    [[ ! "$status" -eq 0 ]]
    echo $output | grep -q "is not installed"
}

@test "system dirs" {
    run rig -q system dirs
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^Mode  *admin$"
    echo "$output" | grep -q "^Architecture  *\(x86_64\|aarch64\)$"
    echo "$output" | grep -q "^R root  *C:.Program Files.R"
    echo "$output" | grep -q "^Rtools root  *C:"
    echo "$output" | grep -q "^Binary dir  *C:.Program Files.R.bin$"

    run rig -q system dirs --json
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q '"rtools_root"'
    echo "$output" | grep -q '"arch"'
}

@test "system dirs, single directory" {
    run rig -q system dirs --r
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^C:.Program Files.R"

    # the admin mode R root is architecture dependent on Windows
    run rig -q system dirs --r --arch x86_64
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    run rig -q system dirs --r --arch arm64
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    [[ "$output" = "$(rig -q system dirs --r --arch aarch64)" ]]

    # Rtools keeps its historical C:\Rtools<version> location in admin mode
    run rig -q system dirs --rtools
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^C:.$"

    run rig -q system dirs --binary
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^C:.Program Files.R.bin$"

    run env RIG_MODE=user rig -q system dirs --rtools
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "rig.data.rtools$"

    # hidden no-op off Linux
    run rig -q system dirs --fonts
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    [[ -z "$output" ]]

    # the selectors are mutually exclusive and cannot be combined with --json
    run rig -q system dirs --r --binary
    echo "status = ${status}"
    echo "output = ${output}"
    [[ ! "$status" -eq 0 ]]
    run rig -q system dirs --r --json
    echo "status = ${status}"
    echo "output = ${output}"
    [[ ! "$status" -eq 0 ]]
}

@test "list" {
    run rig list
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^[*] 4.5.0"
    run rig ls
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^  4.4.3"
}

@test "resolve" {
    run rig resolve devel
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rig resolve release
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rig resolve oldrel
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rig resolve oldrel/1
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "[0-9][.][0-9][.][0-9] https://"
    run rig resolve 4.5.0
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "4[.]5[.]0 https://"
    run rig resolve 4.4
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "4[.]4[.]3 https://"
}

@test "rm" {
    if ! rig ls | grep -q '^[* ] 4.4.2$'; then
        run rig add 4.4.2
	echo "status = ${status}"
	echo "output = ${output}"
        [[ "$status" -eq 0 ]]
        run rig ls
        echo "$output" | grep -q "^[* ] 4[.]4[.]2"
    fi
    run rig rm 4.4.2
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    run rig list
    echo $output | grep -vq "^[* ] 4.4.2"
}

# The quoting is very tricky here. We avoid double quotes because they
# change the Windows parsing rules.

# For the output we take the last line, in case there are warnings at
# startup. (This does happen in bash for R 4.1.1.)

@test "system create-lib" {
    # Must already exist
    run R-4.5.0.bat -q -s -e suppressWarnings\(file.exists\(Sys.getenv\(\'R_LIBS_USER\'\)\)\)
    echo "status = ${status}"
    echo "output = ${output}"
    [[ $status -eq 0 ]]
    [[ "${lines[-1]}" = "[1] TRUE" ]]
    run R-devel.bat -q -s -e file.exists\(Sys.getenv\(\'R_LIBS_USER\'\)\)
    echo "status = ${status}"
    echo "output = ${output}"
    [[ $status -eq 0 ]]
    [[ "${lines[-1]}" = "[1] TRUE" ]]
    run R-4.4.3.bat -q -s -e file.exists\(Sys.getenv\(\'R_LIBS_USER\'\)\)
    echo "status = ${status}"
    echo "output = ${output}"
    [[ $status -eq 0 ]]
    [[ "${lines[-1]}" = "[1] TRUE" ]]
    run rig system create-lib
    echo "status = ${status}"
    echo "output = ${output}"
    [[ $status -eq 0 ]]
}

@test "system add-pak" {
    run rig default 4.5.0
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    run rig system add-pak
    echo $output | grep -qE "(Installing|Updating) pak for R 4.5.0"
    run R-4.5.0.bat -q -s -e 'pak::lib_status()'
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
}

@test "system clean-registry" {
    run rig system clean-registry
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
}

# This is tested implicitly

@test "system make-links" {
    true
}
