## Windows

### 1. Install the latest release

``` powershell
rig add release
```

Admin mode installs into `C:\Program Files\R`, so rig needs your password.

Besides unpacking R, `rig add` also configures the CRAN and
[P3M](https://p3m.dev/) repositories, installs [pak](https://pak.r-lib.org/),
sets R up to use a user package library, creates the quick links, and (if this
is your first R version) makes it the default.

On arm64 Windows rig installs arm64 R builds by default. Use the
`--arch x86_64` flag to install an x86_64 R build. You can mix x86_64 and
arm64 builds of R and Rtools.

### 2. See what you have

``` powershell
rig list
```

```
* name   version  aliases
-------------------------
* 4.6.1           release
```

The `*` marks the default version. `rig list --plain` prints just the names,
one per line, which is handy in scripts.

### 3. Add a second version

R versions are named with a *version spec*, not just a number:

``` powershell
rig add oldrel     # the previous minor release
rig add 4.4.3      # an exact version
rig add 4.4        # the latest patch release of 4.4.x
rig add oldrel/2   # two minor releases back
rig add devel      # the current R-devel
rig add next       # the current R patched / release candidate
```

`rig available` lists everything you can install.

### 4. Switch the default version

``` powershell
rig default          # print the current default
rig default 4.5.3    # set it
```

The default version is the one the plain `R` and `Rscript` commands start. rig
also creates versioned quick links, so you can always reach a specific version
without switching:

``` powershell
R                                # the default version
R-4.5.3                          # a specific version
Rscript -e 'R.version.string'    # the default version, non-interactively
```

::: {.callout-note}
If typing `R` in PowerShell re-runs your previous command instead of starting
R, you have hit PowerShell's built-in `r` alias for `Invoke-History`, which
takes precedence over external commands. Run
[`rig system fix-r-alias`](reference/system.qmd#rig-system-fix-r-alias) once and
open a new PowerShell session.
:::

### 5. Install a package

`rig add` installed pak, so you can install packages straight away:

``` powershell
R -q -e 'pak::pkg_install("dplyr")'
```

Packages go into a per-user, per-version library that rig configured R to
create automatically, so installing packages never needs administrator rights,
and packages for different R versions never collide.

If you want more than one library for the same R version (say one per project),
see the [`rig library`](reference/library.qmd) command.

### 6. Install Rtools

To build packages from source you need Rtools, which rig manages too:

``` powershell
rig rtools list        # what is installed
rig add rtools         # the Rtools version matching your default R
rig rtools add 45      # a specific Rtools version
```

rig configures R and Rtools to work without putting Rtools on the PATH,
so `R CMD config`, `R CMD sh`, `R CMD make` and `rig run --cmd config` work
out of the box.

### 7. Run R without switching the default

[`rig run`](reference/run.qmd) runs R, a script or a whole project with any
installed version, leaving your default alone:

``` powershell
rig run                            # start R
rig run -f analysis.R              # run a script
rig run -e 'sessionInfo()'         # evaluate an expression
rig run -r 4.5.3 -f analysis.R     # ... with a specific R version
rig run --cmd check --no-manual .  # run `R CMD check`
```

### 8. Turn on shell completions

rig ships PowerShell completions. See [Installing
auto-complete](install.qmd#installing-auto-complete) for how to load them from
your profile.
