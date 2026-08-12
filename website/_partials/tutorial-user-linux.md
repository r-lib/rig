# Linux

## 1. Switch rig to user mode

rig starts out in admin mode even when rig itself lives in your home directory,
so tell it to use user mode from now on:

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
Architecture  x86_64
R root        /home/you/.local/share/rig/r
Binary dir    /home/you/.local/bin
...
```

## 2. Install the latest release

```sh
rig add release
```

No password prompt this time: everything is written under your home directory.

In user mode rig always installs a **portable** R build, one that does not
depend on your distribution's packages, picked for your libc (glibc or musl).
Very old glibc versions are not supported; if yours is too old, rig says so
instead of installing a broken R. rig also downloads a CA certificate bundle
and points R at it, so HTTPS works even where the system store is missing or
stale; you can re-run that step with `rig system update-certs`.

These portable builds are newer than the distro-specific builds that admin
mode installs, and they are not as thoroughly tested yet, so you may hit the
occasional rough edge with them. If that is a problem for you and you do have
an administrator account, consider [admin mode](tutorial-admin.qmd) instead.
Either way, please [report](https://github.com/r-lib/rig/issues) what you run
into.

Besides unpacking R, `rig add` also configures the CRAN and
[P3M](https://p3m.dev/) repositories (P3M serves pre-compiled, self-contained
Linux binaries, which makes installing packages faster and more robust), installs
[pak](https://pak.r-lib.org/), sets R up to use a user package library, creates
the quick links, and (if this is your first R version) makes the newly installed
version the default.

## 3. Make sure `~/.local/bin` is on your `PATH`

rig needs `~/.local/bin` on your `PATH` for the `R` and `Rscript` commands to
be found. It sets this up for you: it writes a small `~/.local/bin/rigenv`
snippet and sources it from your shell startup files (`.profile`,
`.bash_profile`, `.bashrc`, `.zprofile`, `.zshrc`, and
`~/.config/fish/conf.d/rigenv.fish` if you use fish).

That only affects *new* shells, so for the session you are in rig tells you how
to catch up:

```
⚠ /home/you/.local/bin is not on the PATH.
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

The quick links are in `~/.local/bin`, so switching the default never needs a
password.

## 7. Install an R package

`rig add` installs pak by default, so you can install R packages straight away:

```sh
R -q -e 'pak::pkg_install("dplyr")'
```

Packages go into a per-user, per-version library that rig configured R to
create automatically, so packages for different R versions never collide.

Many R packages need system libraries to build and run. pak can install those
with your distribution's package manager, but that needs `sudo`, which is
exactly what user mode avoids. rig sets up P3M's manylinux package repository
binaries on glibc based Linux systems to fix this. These binary packages
are self-contained and do not need any system libraries to run. The manylinux
repository is fairly new, though, and not as thoroughly tested as P3M's
distro-specific repositories, so occasional problems with individual packages
are still to be expected.

If P3M is missing a binary for a certain package, then you need to install
this package form source. If the package needs system libraries, then
you'll need to install them yourself, or ask an administrator to install
them.

P3M does not have binary packages for musl libc based Linux distributions
(e.g. Alpine Linux).

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
