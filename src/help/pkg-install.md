Install packages from the repositories

## Note

This command currently only uses PPM (Posit Public Package Manager) and
ignores the configured repositories.

## Description

Install one or more R packages, and everything they need, into an R package
library.

rig resolves the whole dependency tree first, the same way [`rig proj lock`](proj.qmd)
does for a project, so a package is only installed if every package it
needs can be installed with it, at versions that work together. `--dry-run`
runs the resolution and reports what it would install, without installing
anything.

## Git, GitHub, GitLab and URL sources

A package can also be given straight from a git repository, [pak's package
reference](https://pak.r-lib.org/reference/pak_package_sources.html) syntax,
the same one [`rig proj add`](proj.qmd) accepts:

```
rig pkg install r-lib/crayon
rig pkg install r-lib/crayon@84be6207
rig pkg install r-lib/crayon@some-branch
rig pkg install r-lib/crayon#41
rig pkg install r-lib/crayon@*release
rig pkg install gitlab::group/project@main
rig pkg install 'git::https://gitlab.com/example/pkg.git@main'
```

A bare `<owner>/<repo>` (optionally `github::<owner>/<repo>`) is a GitHub
reference; `<owner>/<repo>/<subdir>` points at a package in a subdirectory of
the repository. After the path, `@<ref>` pins a branch, tag or commit,
`#<pr>` a pull request, and `@*release` the latest release.

`gitlab::<group>/<project>` is the same idea for GitLab, including nested
subgroups (`gitlab::<group>/<subgroup>/<project>`); `@<ref>` pins a branch,
tag or commit, and `/-/<subdir>` points at a subdirectory. Merge requests and
`@*release` are not supported for GitLab. A self-hosted instance is
`gitlab::<https-url-of-group-and-project>`, e.g.
`gitlab::https://gitlab.example.com/group/project`.

A `git::<url>` reference works with any git host, not only GitHub or GitLab.

`url::<https-url>` points straight at a package source archive (`.tar.gz`,
`.tgz` or `.zip`), instead of a git repository, e.g.
`url::https://example.com/mypkg_1.0.0.tar.gz`. rig downloads and extracts it
to read its `DESCRIPTION`, and caches the download for later installs.

The package name is read from the fetched repository's (or archive's) own
`DESCRIPTION` (which may differ from the repository name); the dependency is
pinned to an exact commit, or to the archive's sha256 for a `url::` source,
resolved right away, not to a version range.

rig fetches a git/GitHub/GitLab source with the system `git`, which must be
installed and on `PATH`. A private repository authenticates exactly the way
a plain `git clone` would on your machine: a configured credential helper
(Keychain, Windows Credential Manager, `git credential-store`, etc.),
`.netrc`, an SSH agent, or credentials already embedded in the URL. There is
no separate rig-specific token setting. A `url::` source is a plain HTTP(S)
download and needs no such authentication.

## Dev dependencies

By default rig installs the hard dependencies only: `Depends`, `Imports` and
`LinkingTo`, i.e. the packages that need to be installed to use the package.
`--dev` also installs the soft dependencies, `Suggests` and `Enhances`, which are
typically only needed to run the tests, build the vignettes or use some
optional feature.

A package sometimes has dev dependencies that are not in the repositories
rig installs from. Those cannot be installed, and by default rig reports
them and errors. See `--ignore-unavailable` to ignore them.

## Binary and source packages

A binary package is a package that has already been built for your platform
and R version. Installing one is unpacking it into the library, so rig does
that itself and never starts R.

A package with no binary build is installed from its source tarball, with `R
CMD INSTALL`, which does start R, and needs whatever that package needs to
compile. The output of the compilation goes into a log file per package, in
a `_logs` directory inside the library, and rig points at the log when an
installation fails.

`--platform` installs for a platform other than this machine's, and
`--platform source` installs source packages only.

`--prefer-binary` trades a newer version for an older one that has a binary
build, which is useful when compiling is expensive; it takes the number of
versions to look back through, e.g. `--prefer-binary=5`, and defaults to 3.

## Caching package builds

rig caches the packages that it compiles and uses it in subsequent
installations to avoid recompiling the same package. A locally build cached
binary belongs to a specific platform, minor R version, source package, set
of versions of the packages it is compiled against, and set of
`~/.R/Makevars` files. Change any of those and the package is compiled again.

`--no-cache` turns all of this off for one run: rig then downloads the
repository metadata and the package files again, compiles every source
package rather than unpacking one it built earlier, and adds nothing to the
cache. See [`rig config`](config.qmd) .

## Skipping already installed packages

rig does not install a package that is already installed and up to date, so
running the same command twice does nothing the second time.

rig makes sure that installed packages are ABI compatible and reinstalls
packages as needed to make sure that a package with `LinkingTo` dependencies
is build against the installed version of those dependencies.

rig may reinstall more packages than needed if cannot determine `LinkingTo`
compatibility of an installed package, typically this happens if that
package was not installed with rig.

Use `--reinstall` to installs everything in the resolution.

## Which library

By default rig installs into the default library of the default R version,
i.e. the library that [`rig library default`](library.qmd) reports, and the one R installs
packages into.

`--library` (`-l`) selects another library. It takes either the name of a
library of the R version, as [`rig library list`](library.qmd) prints them, or the path of a
library directory. A library name is tried first, so prefix a relative path
with `./` (or use an absolute path) if it happens to share a name with a
library:

```
rig pkg install --library myproject cli
rig pkg install --library /usr/lib/R/site-library cli
```

`--r-version` (`-r`) selects the library of another R version, instead of the
default one, as it does for the [`rig library`](library.qmd) commands.

In [admin mode](../admin-vs-user-mode.qmd) the site and system libraries of an R installation belong to
the administrator, so installing into them needs `sudo` (an administrator
account on Windows). Your own user library never does.
