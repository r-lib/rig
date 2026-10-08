Add a custom R package repository

## Description

Add a CRAN-like package repository to rig, e.g. your company's internal CRAN
mirror, or the r-universe of your organization.

rig stores the repository in its configuration file, so you only need to add it
once. It is not used by any R version until you enable it, either with `--enable`
here or later with [`rig repos enable`](#rig-repos-enable). After that it is just like the repositories
built into rig: it shows up in [`rig repos available`](#rig-repos-available), and you can enable it with
`--with-repos` when you install a new R version with [`rig add`](add.qmd) or call
[`rig repos setup`](#rig-repos-setup).

The name of the repository must start with a letter or a number, and can contain
letters, numbers and `.`, `_`, `/`, `-`. It cannot be the name of a repository that is
built into rig. Use `--force` to replace a custom repository of the same name.

The URL must start with `https://`, `http://` or `file://`. It is the URL that you
would put into the `repos` option in R, i.e. the directory that has the
`src/contrib` directory in it, and the `bin` directory, if the repository has binary
packages.

With `--enable` rig enables the repository for the default R version, or the R
versions you list with `--r-version`, or all of them with `--all-versions`.
`--enable --all-versions` also enables the repository for the R versions you
install later, the same way as CRAN. To undo this, add the repository again
with `--force`, without `--all-versions`. You can still turn it off for some R
versions with [`rig repos disable`](#rig-repos-disable), or for a new R version
with `--without-repos` when installing it.

rig updates the files of the R installation. If you cannot write them, which is
typical in [admin mode](../admin-vs-user-mode.qmd) on Linux and Windows, rig asks for administrator rights,
e.g. your password for `sudo`.

## Examples

```sh
# Add a repository, without enabling it
rig repos add acme https://cran.acme.com --title "Acme CRAN mirror"

# Add a repository and enable it for the default R version
rig repos add acme-universe https://acme.r-universe.dev --enable

# Add a repository and enable it for all R versions, also the ones
# installed later
rig repos add acme https://cran.acme.com --enable --all-versions
```
