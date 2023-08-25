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

@test "add" {
    if ! rig ls | grep -q '^[* ] 4.1.2'; then
	run rig -v add 4.1.2
	[[ "$status" -eq 0 ]]
	run rig ls
	echo "$output" | grep -q "^[* ] 4.1.2"
    fi
    run R-4.1.2 -q -s -e 'cat(as.character(getRversion()))'
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4[.]1[.]2$"

    if ! rig ls | grep -q '^[* ] 4.0.5'; then
	run rig add 4.0
	[[ "$status" -eq 0 ]]
	run rig ls
	echo "$output" | grep -q "^[* ] 4.0.5"
    fi
    run R-4.0.5 -q -s -e 'cat(as.character(getRversion()))'
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^4[.]0[.]5$"

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
    run rig default 4.1.2
    [[ "$status" -eq 0 ]]
    run rig default
    [[ "$output" = "4.1.2" ]]
    run rig default 1.0
    [[ ! "$status" -eq 0 ]]
    echo $output | grep -q "is not installed"
}

@test "list" {
    run rig list
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^[*] 4.1.2"
    run rig ls
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^  4.0.5"
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
    run rig resolve 4.1.1
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "4[.]1[.]1 https://"
    run rig resolve 4.0
    [[ "$status" -eq 0 ]]
    echo $output | grep -q "4[.]0[.]5 https://"
}

@test "rm" {
    if ! rig ls | grep -q '^[* ] 3.3.3$'; then
        run rig add 3.3
        [[ "$status" -eq 0 ]]
        run rig ls
        echo "$output" | grep -q "^[* ] 3[.]3[.]3"
    fi
    run rig rm 3.3.3
    [[ "$status" -eq 0 ]]
    run rig list
    echo $output | grep -vq "^[* ] 3.3.3"
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
    run rig -vv system create-lib
    echo "$output"
    [[ $status -eq 0 ]]
}

@test "system add-pak" {
    if ! rig ls | grep -q '^[* ] 4.1.2'; then
	run rig -v add 4.1.2
	[[ "$status" -eq 0 ]]
    fi
    run rig default 4.1.2
    [[ "$status" -eq 0 ]]
    run rig system add-pak
    echo $output | grep -q "Installing pak for R 4.1.2"
    run R-4.1.2 -q -s -e 'pak::lib_status()'
    [[ "$status" -eq 0 ]]

    if ! rig ls | grep -q '^[* ] 3.4.4$'; then
        run rig add 3.4.4
        [[ "$status" -eq 0 ]]
        run rig ls
        echo "$output" | grep -q "^[* ] 3[.]4[.]4"
    fi

    libdir=`R-3.4.4 -s -e 'cat(path.expand(Sys.getenv("R_LIBS_USER")))'`
    [[ "$libdir" == "" ]] && false
    run $SUDO rm -rf "$libdir"
    run $SUDO `which rig` system add-pak 3.4.4
    [[ "$status" -eq 0 ]]
    uid=`stat -c "%u" "$libdir"`
    [[ "$uid" -eq "`id -u`" ]]
}
