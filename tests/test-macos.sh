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

@test "--no-cache" {
    # accepted before and after the subcommand, and from the environment
    run rig -q --no-cache system dirs --cache
    [[ "$status" -eq 0 ]]
    cache="$output"
    run rig -q system dirs --no-cache --cache
    [[ "$status" -eq 0 ]]
    [[ "$output" = "$cache" ]]
    run env RIG_NO_CACHE=true rig -q system dirs --cache
    [[ "$status" -eq 0 ]]
    # the reported cache directory is the real one either way: --no-cache
    # changes where this run keeps things, not where rig keeps them
    [[ "$output" = "$cache" ]]
    [[ "$output" = "$(rig -q system dirs --cache)" ]]

    # a value that is not a boolean is a warning, not a failure
    run env RIG_NO_CACHE=perhaps rig -q system dirs --cache
    [[ "$status" -eq 0 ]]
    [[ "$output" = "$cache" ]]

    # a command that never looks at the cache does not create a throwaway
    # directory for it either, and one that does cleans it up afterwards
    run bash -c 'ls -d "${TMPDIR:-/tmp}"/rig-nocache-* 2>/dev/null | wc -l'
    [[ "$output" -eq 0 ]]
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

@test "repos add/enable/disable/rm" {
    run rig repos add rigtest https://cloud.r-project.org --title "rig test repo"
    [[ "$status" -eq 0 ]]
    run rig repos available
    echo "$output" | grep -q "^rigtest .*custom"
    run rig repos enable rigtest -r 4.1
    [[ "$status" -eq 0 ]]
    run rig repos list -r 4.1
    echo "$output" | grep -q "^rigtest "
    # choices survive a new setup
    run rig repos setup -r 4.1
    [[ "$status" -eq 0 ]]
    run rig repos list -r 4.1
    echo "$output" | grep -q "^rigtest "
    run rig repos disable rigtest -r 4.1
    [[ "$status" -eq 0 ]]
    run rig repos list -r 4.1
    ! echo "$output" | grep -q "^rigtest "
    run rig repos enable rigtest -r 4.1
    run rig repos rm rigtest
    [[ "$status" -eq 0 ]]
    run rig repos list -r 4.1
    ! echo "$output" | grep -q "^rigtest "
    run rig repos rm cran
    [[ ! "$status" -eq 0 ]]
}

@test "rig pkg uses the configured repositories" {
    # Clean up before checking anything, so a failure does not leave the
    # repository setup changed for the other tests.
    run rig repos add rigtest https://cran.r-project.org --enable -r 4.1
    add_status=$status
    run rig pkg deps cli --r-version 4.1
    deps_status=$status
    deps_output=$output
    rig repos rm rigtest
    [[ "$add_status" -eq 0 ]]
    [[ "$deps_status" -eq 0 ]]
    echo "$deps_output" | grep -q "metadata of repository rigtest"
}

@test "add --json" {
    # Already installed, so nothing is installed, only reported.
    run bash -c "rig add --json 4.0.5 -a x86_64 2>/dev/null"
    echo "status = ${status}"
    echo "output = ${output}"
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q '"name": "4.0"'
    echo "$output" | grep -q '"version": "4.0.5"'
    echo "$output" | grep -q '"default": false\(,\|$\)'
    echo "$output" | grep -q '"new-install": false\(,\|$\)'
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

@test "system blas" {
    run sudo rig system blas set accelerate 4.1
    [[ $status -eq 0 ]]
    run readlink /Library/Frameworks/R.framework/Versions/4.1/Resources/lib/libRblas.dylib
    [[ "$output" == "libRblas.vecLib.dylib" ]]
    run rig system blas status 4.1
    [[ $status -eq 0 ]]
    echo $output | grep -q -- "accelerate"

    run sudo rig system blas set reference 4.1
    [[ $status -eq 0 ]]
    run readlink /Library/Frameworks/R.framework/Versions/4.1/Resources/lib/libRblas.dylib
    [[ "$output" == "libRblas.0.dylib" ]]
    run rig system blas status 4.1
    [[ $status -eq 0 ]]
    echo $output | grep -q -- "reference"
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

@test "proj init" {
    cd "$BATS_TEST_TMPDIR"
    rm -rf myproj && mkdir myproj && cd myproj

    # No R needs to be installed for the requested version, `rig proj init`
    # does not touch an R installation.
    run rig proj init -r 4.1
    [[ "$status" -eq 0 ]]
    [[ -f rproj.toml ]]
    [[ -f .Renviron ]]
    [[ -f .gitignore ]]
    [[ -f .rvenvlib/rvenv/DESCRIPTION ]]
    grep -q '^name = "myproj"$' rproj.toml
    grep -q '^R = ">= 4.1"$' rproj.toml
    grep -q '^R_LIBS_USER=.rvenvlib$' .Renviron
    grep -q '^/.rvenv/$' .gitignore
    grep -q '^Package: rvenv$' .rvenvlib/rvenv/DESCRIPTION
    # The project library is `rig proj sync`'s to create
    [[ ! -d .rvenv/lib ]]

    # The IDE leg: a plain R session in the project picks up the shim
    # package. Without a project library it warns and leaves the library path
    # as it was, instead of pointing the session at rig's own directory.
    run env -u RVENV R-4.1 -q -s -e 'cat(.libPaths()[1], Sys.getenv("R_LIBS_USER"))'
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "Project is not synced"
    [[ "$output" != *".rvenv"* ]]

    # Once the library exists, the shim resolves it and puts it first. The
    # rest of activation lives in `onload.R`, normally written by `rig proj
    # sync`; copy it in here to simulate a synced project without a full sync.
    mkdir -p .rvenv/lib
    cp "$DIR/../src/data/rvenv/onload.R" .rvenv/onload.R
    run env -u RVENV R-4.1 -q -s -e 'cat(.libPaths()[1])'
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "myproj/[.]rvenv/lib"
    rm .rvenv/onload.R
    rmdir .rvenv/lib

    # Refuses to overwrite, and says what is in the way
    run rig proj init -r 4.1
    [[ "$status" -ne 0 ]]
    echo "$output" | grep -q "rproj.toml"
    echo "$output" | grep -q -- "--force"

    # --force keeps the user's own ignore rules, rig only manages its block
    echo "*.log" >> .gitignore
    run rig proj init -r 4.1 --force
    [[ "$status" -eq 0 ]]
    grep -q '^[*].log$' .gitignore
    [[ "$(grep -c '^# rig rvenv start$' .gitignore)" -eq 1 ]]
}

@test "proj import" {
    cd "$BATS_TEST_TMPDIR"
    rm -rf impproj && mkdir impproj && cd impproj

    cat > DESCRIPTION <<-EOF
	Package: impproj
	Version: 1.2.3
	Title: A Test Package
	Depends: R (>= 4.1)
	Imports: jsonlite
	EOF

    # A full import sets up the whole project, not just the manifest.
    run rig proj import
    [[ "$status" -eq 0 ]]
    [[ -f rproj.toml ]]
    [[ -f .Renviron ]]
    [[ -f .gitignore ]]
    [[ -f .rvenvlib/rvenv/DESCRIPTION ]]
    grep -q '^name = "impproj"$' rproj.toml
    grep -q '^version = "1.2.3"$' rproj.toml
    grep -q '^R = ">= 4.1"$' rproj.toml
    grep -q '^jsonlite = ' rproj.toml

    # Refuses to overwrite the manifest, and says what to do instead
    run rig proj import
    [[ "$status" -ne 0 ]]
    echo "$output" | grep -q "rproj.toml"
    echo "$output" | grep -q -- "--dependencies"

    # Refuses to overwrite the .rvenv files, and says what is in the way
    rm rproj.toml
    run rig proj import
    [[ "$status" -ne 0 ]]
    echo "$output" | grep -q ".Renviron"
    echo "$output" | grep -q -- "--force"

    run rig proj import --force
    [[ "$status" -eq 0 ]]

    # --dependencies only writes the manifest
    cd "$BATS_TEST_TMPDIR"
    rm -rf impdeps && mkdir impdeps && cd impdeps
    cp ../impproj/DESCRIPTION .
    run rig proj import --dependencies
    [[ "$status" -eq 0 ]]
    [[ -f rproj.toml ]]
    [[ ! -e .rvenv ]]
    [[ ! -e .Renviron ]]
}

@test "proj import remotes" {
    cd "$BATS_TEST_TMPDIR"
    rm -rf impremotes && mkdir impremotes && cd impremotes

    cat > DESCRIPTION <<-EOF
	Package: impremotes
	Version: 1.0.0
	Title: A Test Package
	Imports: crayon (>= 1.5.0), limma
	Remotes: r-lib/crayon@main, bioc::limma, bioc::biocpkg, bitbucket::user/repo
	EOF

    run rig proj import --dependencies
    [[ "$status" -eq 0 ]]
    [[ -f rproj.toml ]]
    grep -q 'git = "https://github.com/r-lib/crayon.git"' rproj.toml
    grep -q 'rev = "main"' rproj.toml
    # The version requirement from Imports is kept alongside the git source.
    grep -q 'version = ">= 1.5.0"' rproj.toml
    # A `bioc::` remote is the same as the plain package name: ignored,
    # without a warning, whether the package is a dependency or not.
    grep -q '^limma = "\*"$' rproj.toml
    ! grep -q 'repository = "bioc"' rproj.toml
    ! echo "$output" | grep -q "bioc::"
    ! grep -q 'biocpkg' rproj.toml
    # Unsupported remote type: warned about, not written as a git source, and
    # does not fail the import.
    echo "$output" | grep -q "bitbucket::user/repo"
    ! grep -q 'bitbucket' rproj.toml
}

@test "proj add" {
    cd "$BATS_TEST_TMPDIR"
    rm -rf addproj && mkdir addproj && cd addproj
    run rig proj init -r 4.1
    [[ "$status" -eq 0 ]]

    # --no-lock only edits the manifest, so none of this needs the network
    run rig proj add praise --no-lock
    [[ "$status" -eq 0 ]]
    grep -q '^praise = "\*"$' rproj.toml

    # a bare version means "compatible with", written out as such
    run rig proj add jsonlite@1.8.0 --no-lock
    [[ "$status" -eq 0 ]]
    grep -q '^jsonlite = "\^1.8.0"$' rproj.toml

    # --dev adds to the dev dependency group
    run rig proj add 'testthat@>= 3.0' --dev --no-lock
    [[ "$status" -eq 0 ]]
    grep -q '^\[dependency-groups.dev\]$' rproj.toml
    grep -q '^testthat = ">= 3.0"$' rproj.toml

    # adding a package again updates its version requirement
    run rig proj add 'testthat@>= 3.2' --dev --no-lock
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "Updated testthat"
    grep -q '^testthat = ">= 3.2"$' rproj.toml

    # a version requirement that does not parse is refused, and the manifest
    # is left alone
    run rig proj add 'praise@nope' --no-lock
    [[ "$status" -ne 0 ]]
    grep -q '^praise = "\*"$' rproj.toml
}

@test "run script with inline dependencies" {
    cd "$BATS_TEST_TMPDIR"
    rm -rf scriptdir && mkdir scriptdir && cd scriptdir

    # a plain script runs like `rig run -f`
    printf 'cat(commandArgs(TRUE), "\\n")\n' > plain.R
    run rig run plain.R a b
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^a b"

    # Rscript gets the arguments without an extra `--args`
    run rig run --rscript plain.R a b
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^a b"
    run rig run --rscript -e 'cat(commandArgs(TRUE), "\n")' a b
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^a b"

    # a script with a #! line can have any name, and gets all arguments
    # after it, even the ones that look like flags
    printf '#!/usr/bin/env -S rig run\ncat(commandArgs(TRUE), "\\n")\n' > shebang
    chmod +x shebang
    run ./shebang --foo -- bar --help
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q -- "^--foo -- bar --help"

    cat > deps.R <<'SCRIPT'
# /// script
# [dependencies]
# R = ">= 4.1"
# praise = "*"
# ///
cat("praise", format(packageVersion("praise")), "\n")
cat("libpath", .libPaths()[1], "\n")
cat("args", commandArgs(TRUE), "\n")
SCRIPT
    run rig run deps.R x
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^praise "
    echo "$output" | grep -q "^libpath .*scripts"
    echo "$output" | grep -q "^args x"

    # the second run reuses the environment
    run rig run deps.R
    [[ "$status" -eq 0 ]]
    ! echo "$output" | grep -q "Setting up the environment"

    # --upgrade-package solves the environment again
    run rig run -P praise deps.R
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "Re-locking the environment"
    echo "$output" | grep -q "^praise "

    # the upgrade flags only work for scripts with a block
    run rig run --upgrade plain.R
    [[ "$status" -ne 0 ]]
    echo "$output" | grep -q "only work for scripts"

    # setting up the environment writes nothing to stdout, that is the
    # script's. --no-cache forces a new environment.
    printf '# /// script\n# [dependencies]\n# praise = "*"\n# ///\ncat("only this\\n")\n' > quiet.R
    out="$(rig --no-cache run quiet.R 2>/dev/null)"
    [[ "$out" == "only this" ]]

    # a broken block is an error
    printf '# /// script\n# [dependencies]\n1\n' > bad.R
    run rig run bad.R
    [[ "$status" -ne 0 ]]
    echo "$output" | grep -q "does not start with"
}

@test "proj init/add/remove --script" {
    cd "$BATS_TEST_TMPDIR"
    rm -rf scriptedit && mkdir scriptedit && cd scriptedit

    # init adds a block after the shebang, and refuses to replace it
    printf '#!/usr/bin/env -S rig run\ncat("praise", format(packageVersion("praise")), "\\n")\n' > s.R
    run rig proj init --script s.R
    [[ "$status" -eq 0 ]]
    [[ "$(sed -n 2p s.R)" == "# /// script" ]]
    grep -q '^# R = ">= ' s.R
    run rig proj init --script s.R
    [[ "$status" -ne 0 ]]
    echo "$output" | grep -q "already has"
    [[ ! -e rproj.toml ]]

    # init creates a missing script
    run rig proj init --script new.R
    [[ "$status" -eq 0 ]]
    [[ "$(head -1 new.R)" == "# /// script" ]]

    # add edits the block and sets up the environment
    run rig proj add --script s.R praise
    [[ "$status" -eq 0 ]]
    grep -q '^# praise = "\*"$' s.R
    run rig run s.R
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^praise "
    ! echo "$output" | grep -q "Setting up the environment"

    # a package that does not exist leaves the script alone
    cp s.R s.R.orig
    run rig proj add --script s.R notapackageatall
    [[ "$status" -ne 0 ]]
    cmp s.R s.R.orig

    # remove edits the block, a missing name is an error
    run rig proj remove --script s.R praise --no-lock
    [[ "$status" -eq 0 ]]
    ! grep -q 'praise = ' s.R
    run rig proj remove --script s.R praise --no-lock
    [[ "$status" -ne 0 ]]
    echo "$output" | grep -q "Not a dependency"
}

@test "script lock files" {
    cd "$BATS_TEST_TMPDIR"
    rm -rf scriptlock && mkdir scriptlock && cd scriptlock

    # a script without a block cannot be locked
    printf 'cat("hi\\n")\n' > plain.R
    run rig proj lock --script plain.R
    [[ "$status" -ne 0 ]]
    echo "$output" | grep -q "has no"
    [[ ! -e plain.R.lock ]]

    cat > s.R <<'SCRIPT'
# /// script
# [dependencies]
# praise = "*"
# ///
cat("praise", format(packageVersion("praise")), "\n")
SCRIPT
    run rig proj lock --script s.R
    [[ "$status" -eq 0 ]]
    grep -q 'package = "praise"' s.R.lock
    [[ "$(grep -c '^\[\[targets\]\]' s.R.lock)" -gt 1 ]]

    # run uses the lock file, and does not change it
    cp s.R.lock s.R.lock.orig
    run rig run s.R
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^praise "
    cmp s.R.lock s.R.lock.orig
    run rig run --locked s.R
    [[ "$status" -eq 0 ]]

    # a changed block does not fit the lock file any more: --locked fails,
    # a plain run updates the lock file
    cat > s.R <<'SCRIPT'
# /// script
# [dependencies]
# praise = "*"
# glue = "*"
# ///
cat("praise", format(packageVersion("praise")), "\n")
SCRIPT
    run rig run --locked s.R
    [[ "$status" -ne 0 ]]
    echo "$output" | grep -q "does not fit"
    cmp s.R.lock s.R.lock.orig
    run rig run s.R
    [[ "$status" -eq 0 ]]
    echo "$output" | grep -q "^praise "
    grep -q 'package = "glue"' s.R.lock
    run rig run --locked s.R
    [[ "$status" -eq 0 ]]

    # --locked only works for scripts with a block
    run rig run --locked plain.R
    [[ "$status" -ne 0 ]]
}
