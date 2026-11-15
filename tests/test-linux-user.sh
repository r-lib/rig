#!/usr/bin/env bats

# User-mode tests for Linux. They run as a regular (non-root) user, with
# RIG_MODE=user, on stock distro containers. See test-linux-user-docker.sh.

setup() {
    R_ROOT="$HOME/.local/share/rig/r"
    BIN_DIR="$HOME/.local/bin"
}

# The portable R build rig must pick for this system's glibc.
expected_platform() {
    local glibc
    glibc=$(getconf GNU_LIBC_VERSION 2>/dev/null | cut -d" " -f2)
    if [[ -z "$glibc" ]]; then
        glibc=$(ldd --version | head -1 | grep -oE "[0-9]+[.][0-9]+$")
    fi
    if [[ "$(printf '%s\n2.34\n' "$glibc" | sort -V | head -1)" = "2.34" ]]; then
        echo "linux-manylinux-2.34"
    else
        echo "linux-manylinux-2.28"
    fi
}

@test "runs as a regular user" {
    [[ "$(id -u)" -ne 0 ]]
    if command -v sudo >/dev/null 2>&1; then
        run sudo -n true
        [[ ! "$status" -eq 0 ]]
    fi
}

@test "empty" {
    run rig ls
    [[ "$status" -eq 0 ]]
}

@test "system dirs" {
    run rig -q system dirs --r
    [[ "$status" -eq 0 ]]
    [[ "$output" = "$R_ROOT" ]]
    run rig -q system dirs --binary
    [[ "$status" -eq 0 ]]
    [[ "$output" = "$BIN_DIR" ]]
}

@test "--user works without RIG_MODE" {
    run env -u RIG_MODE rig -q --user system dirs --r
    [[ "$status" -eq 0 ]]
    [[ "$output" = "$R_ROOT" ]]
}

@test "add" {
    release=$(rig resolve release | cut -f1 -d" ")
    if ! rig ls | grep -Eq "^[* ] $release( |\$)"; then
        run rig add release
        echo "$output"
        [[ "$status" -eq 0 ]]
    fi
    run rig ls
    echo "$output" | grep -Eq "^[* ] $release( |\$)"
    [[ -d "$R_ROOT/$release" ]]
    run "R-$release" -q -s -e 'cat(as.character(getRversion()))'
    [[ "$status" -eq 0 ]]
    [[ "$output" = "$release" ]]

    oldrel=$(rig resolve oldrel | cut -f1 -d" ")
    if ! rig ls | grep -Eq "^[* ] $oldrel( |\$)"; then
        run rig add oldrel --without-pak
        echo "$output"
        [[ "$status" -eq 0 ]]
    fi
    run rig ls
    echo "$output" | grep -Eq "^[* ] $oldrel( |\$)"
    run "R-$oldrel" -q -s -e 'cat(as.character(getRversion()))'
    [[ "$status" -eq 0 ]]
    [[ "$output" = "$oldrel" ]]
}

@test "the portable build matches the system glibc" {
    release=$(rig resolve release | cut -f1 -d" ")
    expected=$(expected_platform)
    echo "expected: $expected"
    cat "$R_ROOT/$release/metadata.json"
    grep -q "\"platform\": \"$expected\"" "$R_ROOT/$release/metadata.json"
}

@test "nothing is installed outside of the home directory" {
    [[ ! -e /opt/R ]]
    [[ ! -e /usr/local/bin/R ]]
    [[ ! -e /usr/local/bin/Rscript ]]
    run bash -c 'ls /usr/local/bin/R-* 2>/dev/null'
    [[ -z "$output" ]]
}

@test "default" {
    release=$(rig resolve release | cut -f1 -d" ")
    run rig default "$release"
    [[ "$status" -eq 0 ]]
    run rig -q default
    [[ "$status" -eq 0 ]]
    [[ "$output" = "$release" ]]
    run rig list
    echo "$output" | grep -Eq "^[*] $release( |\$)"

    [[ "$(command -v R)" = "$BIN_DIR/R" ]]
    [[ "$(command -v Rscript)" = "$BIN_DIR/Rscript" ]]
    run Rscript -e 'cat(R.home())'
    [[ "$status" -eq 0 ]]
    [[ "$output" == "$R_ROOT/$release"* ]]

    run rig default 1.0
    [[ ! "$status" -eq 0 ]]
    echo "$output" | grep -q "is not installed"
}

@test "user library is in the home directory" {
    run Rscript -e 'cat(.libPaths()[1])'
    [[ "$status" -eq 0 ]]
    [[ "$output" == "$HOME"/* ]]
}

@test "https downloads work" {
    run Rscript -e 'x <- readLines("https://cloud.r-project.org/src/contrib/PACKAGES", n = 1); cat(x)'
    echo "$output"
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^Package:"
}

@test "install a package" {
    # Some minimal images (e.g. OpenSUSE) do not have tar, use R's own.
    if [[ ! -x /usr/bin/tar ]]; then export TAR=internal; fi
    run Rscript -e 'install.packages("R6", repos = "https://cloud.r-project.org"); library(R6)'
    echo "$output"
    [[ "$status" -eq 0 ]]
}

@test "pak works" {
    run Rscript -e 'library(pak); cat(as.character(packageVersion("pak")))'
    echo "$output"
    [[ "$status" -eq 0 ]]

    # This runs in the pak subprocess, and downloads from the repositories.
    run Rscript -e 'st <- pak::repo_status(); print(st[, c("name", "url", "ok")]); stopifnot(all(st$ok[st$name == "CRAN"]))'
    echo "$output"
    [[ "$status" -eq 0 ]]
}

@test "render text with the cairo device" {
    for opt in "" --vanilla; do
        run Rscript $opt -e 'f <- tempfile(fileext = ".png"); png(f); plot(1); text(1, 1, "hello"); dev.off(); stopifnot(file.info(f)$size > 1000)'
        echo "$output"
        [[ "$status" -eq 0 ]]
    done
}

@test "rm" {
    oldrel=$(rig resolve oldrel | cut -f1 -d" ")
    run rig rm "$oldrel"
    echo "$output"
    [[ "$status" -eq 0 ]]
    run rig ls
    if echo "$output" | grep -Eq "^[* ] $oldrel( |\$)"; then false; fi
    [[ ! -e "$R_ROOT/$oldrel" ]]
    [[ ! -e "$BIN_DIR/R-$oldrel" ]]
}
