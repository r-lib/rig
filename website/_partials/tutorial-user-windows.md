# Windows

## 1. Switch rig to user mode

rig starts out in admin mode even when rig itself lives in your user profile,
so tell it to use user mode from now on:

``` powershell
rig system user-mode
```

This is the command the [install script](install.qmd) points you at when it
finishes. On a machine with no system-wide R it has nothing to migrate or clean
up, so it just records the setting and does not raise a UAC prompt.

Check where things will go from now on:

``` powershell
rig system dirs
```

```
Mode          user
Architecture  x86_64
R root        C:\Users\you\AppData\Roaming\rig\data\r
Binary dir    C:\Users\you\.local\bin
...
```

## 2. Install the latest release

``` powershell
rig add release
```

No UAC prompt this time: everything is written under your user profile.

Besides unpacking R, `rig add` also configures the CRAN and
[P3M](https://p3m.dev/) repositories, installs [pak](https://pak.r-lib.org/),
sets R up to use a user package library, creates the quick links, and (if this
is your first R version) makes the newly installed version the default.

On arm64 Windows rig installs arm64 R builds by default. Use the
`--arch x86_64` flag to install an x86_64 R build. You can mix x86_64 and
arm64 builds of R and Rtools.

## 3. Restart your terminal

rig adds the quick-link directory to your user `PATH` in the registry, and
tells you that this needs a fresh terminal:

```
▶ Added C:\Users\you\.local\bin to user PATH
⚠ Restart your terminal (or sign out and back in) for the PATH change to take effect.
```

Open a new terminal, then check that it worked:

``` powershell
Get-Command R
R -q -e 'R.home()'
```

::: {.callout-note}
If typing `R` in PowerShell re-runs your previous command instead of starting
R, you have hit PowerShell's built-in `r` alias for `Invoke-History`, which
takes precedence over external commands. Run
[`rig system fix-r-alias`](reference/system.qmd#rig-system-fix-r-alias) once and
open a new PowerShell session.
:::

## 4. See what you have

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

## 5. Add a second version

R versions can be named with a *version spec*, not just a version number:

``` powershell
rig add 4.4.3      # an exact version
rig add 4.4        # the latest patch release of 4.4.x
rig add oldrel     # the previous minor release
rig add oldrel/2   # two minor releases back
rig add devel      # the current development version of R
rig add next       # the current R patched / release candidate
```

`rig available` lists the R versions you can install, but it leaves out
intermediate patch releases. Use `rig available --all` to include
everything.

## 6. Switch the default version

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

The quick links are in your user profile, so switching the default never raises
a UAC prompt.

## 7. Install an R package

`rig add` installs pak by default, so you can install R packages straight away:

``` powershell
R -q -e 'pak::pkg_install("dplyr")'
```

Packages go into a per-user, per-version library that rig configured R to
create automatically, so packages for different R versions never collide.

If you want more than one library for the same R version, e.g. one per project,
see the [`rig library`](reference/library.qmd) command.

## 8. Install Rtools

To build packages from source you need Rtools, which rig manages too, and in
user mode installs into `%APPDATA%\rig\data\rtools`:

``` powershell
rig rtools list    # what is installed
rig add rtools     # every Rtools version your R versions need
rig add rtools45   # a specific Rtools version
```

rig configures R and Rtools to work without putting Rtools on the PATH,
so `R CMD config`, `R CMD sh`, `R CMD make` and `rig run --cmd config` work
out of the box.

## 9. Run R without switching the default

Another way to run a specific version without changing the default is
[`rig run`](reference/run.qmd). It runs R, a script or a whole project with any
installed version, leaving your default alone:

``` powershell
rig run                            # start R
rig run -f analysis.R              # run a script
rig run -e 'sessionInfo()'         # evaluate an expression
rig run -r 4.5.3 -f analysis.R     # ... with a specific R version
rig run --cmd check --no-manual .  # run `R CMD check`
```

`rig run` also works if R is not on your `PATH`, but `rig` is, which is handy
before you have restarted your terminal.

## 10. Turn on shell completions

rig ships PowerShell completions. Dot-source the script from your PowerShell
profile; for a user-mode install that is:

``` powershell
. "$env:USERPROFILE\.local\share\rig\_rig.ps1"
```

See [Installing auto-complete](install.qmd#installing-auto-complete) for the
details.
