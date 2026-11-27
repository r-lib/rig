List or search the packages of a repository

## Description

List the packages of the R and Bioconductor repositories of the instance,
in the order [`rig ppm repos`](#rig-ppm-repos) lists them, or of the repository `--repo` names.
The `repo` column shows the repository of each package. `--search` keeps only
packages whose name contains the given text, exact matches first.

At most 100 packages are listed in total, use `--limit` for more.

`--fields` selects which fields to show, comma separated. The default is
`name,version,repo,title`. Other useful fields are `license`, `maintainer`,
`depends`, `imports`, `needs_compilation`, `system_requirements`, `package_size` and
`blocked`. With `--json` only the selected fields are printed, too.

`--source` lists the packages of a single source instead of a repository, see
[`rig ppm sources`](#rig-ppm-sources). Looking up a source by name needs a token, a source id
does not.

For Bioconductor repositories rig uses the newest Bioconductor version,
select another one with `--bioc-version`.

## Examples

```sh
rig ppm pkg list --search tidy
rig ppm pkg list --search dplyr --fields name,version,license --limit 5
rig ppm pkg list --repo bioconductor --bioc-version 3.21 --limit 10
```
