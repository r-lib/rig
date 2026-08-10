# macOS

## 1. Switch rig to user mode

rig starts out in admin mode even when rig is installed into your home
directory, so tell it to use user mode from now on:

```sh
rig system user-mode
```

This is the command the [install script](install.qmd) points you at when it
finishes. On a machine with no system-wide R it has nothing to migrate or clean
up, so it just records the setting and does not ask for your password.

Check where things will go from now on:

```sh
rig system dirs
```

```
Mode          user
Architecture  arm64
R root        /Users/you/.local/share/rig/r
Binary dir    /Users/you/.local/bin
...
```

## 2. Install the latest release

```sh
rig add release
```

R is installed under your home directory.
Besides unpacking R, `rig add` also configures the CRAN repository, installs
[pak](https://pak.r-lib.org/) for installing packages, sets R up to use a user
package library, creates the quick links, and (if this is your first R
version) makes the newly installed version the default.

## 3. Make sure `~/.local/bin` is on your `PATH`

rig needs `~/.local/bin` on your `PATH` for the `R` and `Rscript` commands to
be found. It sets this up for you: it writes a small `~/.local/bin/rigenv`
snippet and sources it from your shell startup files (`.profile`,
`.bash_profile`, `.bashrc`, `.zprofile`, `.zshrc`, and
`~/.config/fish/conf.d/rigenv.fish` if you use fish).

That only affects *new* shells, so for the session you are in rig tells you how
to catch up:

```
⚠ /Users/you/.local/bin is not on the PATH.
  To add it to the current session, run:
    . "$HOME/.local/bin/rigenv"                # bash/zsh/sh
    fish_add_path "$HOME/.local/bin"           # fish
  New shell sessions will pick it up automatically.
```

Check that it worked:

```sh
which R
R -q -e 'R.home()'
```

## 4. See what you have

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

## 5. Add a second version

R versions can be named with a *version spec*, not just a version number:

```sh
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

The quick links are in `~/.local/bin`.

## 7. Install an R package

`rig add` installs pak by default, so you can install R packages straight away:

```sh
R -q -e 'pak::pkg_install("dplyr")'
```

Packages go into a per-user, per-version library that rig configured R to
create automatically, so packages for different R versions never collide.

If you want more than one library for the same R version, e.g. one per project,
see the [`rig library`](reference/library.qmd) command.

## 8. Run R without switching the default

Another way to run a specific version without changing the default is
[`rig run`](reference/run.qmd). It runs R, a script or a whole project with any
installed version, leaving your default alone:

```sh
rig run                            # start R
rig run -f analysis.R              # run a script
rig run -e 'sessionInfo()'         # evaluate an expression
rig run -r 4.5.3 -f analysis.R     # ... with a specific R version
rig run --cmd check --no-manual .  # run `R CMD check`
```

`rig run` also works if R is not on your `PATH`, but `rig` is, which makes it a
good fallback if you would rather not set up your `PATH` at all.

## 9. Turn on shell completions

rig ships completions for bash, zsh, fish and elvish. In user mode they are
installed under the install prefix, e.g. `~/.local/share`; see [Installing
auto-complete](install.qmd#installing-auto-complete) for how to load them.

## 10. macOS extras

- The [menu bar app](macos-app.qmd) is not part of the user-mode archive, since
  it installs into `/Applications`.
- To debug R with `lldb` or `gdb`, run `rig system allow-debugger`. The CRAN
  builds are hardened against debuggers by default. In user mode this needs no
  password. There is also `rig system allow-debugger-rstudio` for RStudio's
  `rsession`, and `rig system allow-core-dumps`.
