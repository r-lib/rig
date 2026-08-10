## macOS

### 1. Install the latest release

```sh
rig add release
```

Because admin mode installs into `/Library/Frameworks/R.framework`, rig needs
administrator rights, and asks for your password:

```
Running `sudo` for adding new R versions. This might need your password.
```

Besides unpacking R, `rig add` also configures the CRAN repository, installs
[pak](https://pak.r-lib.org/) for installing packages, sets R up to use a user
package library, creates the quick links, and (if this is your first R
version) makes the newly installed version the default.

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

R versions can be named with a *version spec*, not just a version number:

```sh
rig add 4.4.3      # an exact version
rig add 4.4        # the latest patch release of 4.4.x
rig add oldrel     # the previous minor release
rig add oldrel/2   # two minor releases back
rig add devel      # the current development version of R
rig add next       # the current R patched / release candidate
```

`rig available` lists everything you can install.

::: {.callout-note}
In admin mode on macOS you cannot have two patch releases of the same minor
version at once. Installing R 4.6.1 removes R 4.6.0, because they share a
directory in the R framework. Adding versions from different minor branches
(`release` and `oldrel`, say) is fine. User mode does not have this
restriction.
:::

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

In admin mode `rig default` works without `sudo` as long as your account is in
the `admin` group.

### 5. Install an R package

`rig add` installs pak by default, so you can install R packages straight away:

```sh
R -q -e 'pak::pkg_install("dplyr")'
```

Packages go into a per-user, per-version library that rig configured R to
create automatically, so you do not need `sudo` to install packages even in
admin mode, and packages for different R versions never collide. On macOS rig
also restricts the permissions of the *system* library, so you cannot install
packages into it by accident.

If you want more than one library for the same R version, e.g. one per project,
see the [`rig library`](reference/library.qmd) command.

### 6. Run R without switching the default

Another way to run a specific version without changing the default is
[`rig run`](reference/run.qmd). [`rig run`](reference/run.qmd) runs R, a
script or a whole project with any installed version, leaving your default alone:

```sh
rig run                            # start R
rig run -f analysis.R              # run a script
rig run -e 'sessionInfo()'         # evaluate an expression
rig run -r 4.5.3 -f analysis.R     # ... with a specific R version
rig run --cmd check --no-manual .  # run `R CMD check`
```

`rig run` also works if R is not on your `PATH`, but `rig` is.

### 7. Turn on shell completions

rig ships completions for bash, zsh, fish and elvish. See [Installing
auto-complete](install.qmd#installing-auto-complete) for the setup, which
depends on how you installed rig.

### 8. macOS extras

- The [menu bar app](macos-app.qmd) shows your R versions in the macOS menu
  bar and lets you switch the default with a click. It comes with the system
  installer and `brew install r-rig-app`. Start it once from Finder or with
  `open -a Rig`, then tick "Launch at login".
- To debug R with `lldb` or `gdb`, run `rig system allow-debugger`. The CRAN
  builds are hardened against debuggers by default. There is also
  `rig system allow-debugger-rstudio` for RStudio's `rsession`, and
  `rig system allow-core-dumps`.
