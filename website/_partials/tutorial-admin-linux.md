## Linux

### 1. Install the latest release

```sh
rig add release
```

Admin mode installs into `/opt/R`, so rig needs root, and re-runs itself under
`sudo`:

```
Running `sudo` for adding new R versions. This might need your password.
```

rig installs a build made for your distribution. If there is none for your
platform it falls back to a portable build and says so:

```
No distro-specific R build for platform `...`, falling back to the portable
build `...`.
```

See the [supported
distributions](install.qmd#id-supported-linux-distributions) for the list.

Besides unpacking R, `rig add` also configures the CRAN and
[P3M](https://p3m.dev/) repositories (P3M serves pre-compiled Linux binaries,
which makes installing packages much faster), installs
[pak](https://pak.r-lib.org/), sets R up to use a user package library, creates
the quick links, and (if this is your first R version) makes it the default.

### 2. See what you have

```sh
rig list
```

```
* name   version  aliases
-------------------------
* 4.6.1           release
```

The `*` marks the default version. `rig list --plain` prints just the names,
one per line, which is handy in shell scripts.

### 3. Add a second version

R versions are named with a *version spec*, not just a number:

```sh
rig add oldrel     # the previous minor release
rig add 4.4.3      # an exact version
rig add 4.4        # the latest patch release of 4.4.x
rig add oldrel/2   # two minor releases back
rig add devel      # the current R-devel
rig add next       # the current R patched / release candidate
```

`rig available` lists everything you can install, and
`rig available --list-distros` the Linux distributions rig supports.

### 4. Switch the default version

```sh
rig default          # print the current default
rig default 4.5.3    # set it
```

The default version is the one the plain `R` and `Rscript` commands start. rig
also creates versioned quick links, so you can always reach a specific version
without switching:

```sh
R                              # the default version
R-4.5.3                        # a specific version
Rscript -e 'R.version.string'  # the default version, non-interactively
```

### 5. Install a package

`rig add` installed pak, so you can install packages straight away:

```sh
R -q -e 'pak::pkg_install("dplyr")'
```

Packages go into a per-user, per-version library that rig configured R to
create automatically, so you do not need `sudo` to install packages even in
admin mode, and packages for different R versions never collide.

Many R packages need system libraries to build. pak installs those
automatically on supported distributions, asking for your password when it has
to use the system package manager. Pass `rig add --without-sysreqs` if you
would rather manage system dependencies yourself.

If you want more than one library for the same R version (say one per project),
see the [`rig library`](reference/library.qmd) command.

### 6. Run R without switching the default

[`rig run`](reference/run.qmd) runs R, a script or a whole project with any
installed version, leaving your default alone:

```sh
rig run                          # start R
rig run -f analysis.R            # run a script
rig run -e 'sessionInfo()'       # evaluate an expression
rig run -r 4.5.3 -f analysis.R   # ... with a specific R version
rig run --cmd check --no-manual .  # run `R CMD check`
```

### 7. Turn on shell completions

rig ships completions for bash, zsh, fish and elvish. See [Installing
auto-complete](install.qmd#installing-auto-complete) for the setup, which
depends on how you installed rig.
