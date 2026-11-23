List the repositories of Package Manager

## Description

List the repositories of the Package Manager instance: their names, ids,
types, whether they need a token, and their URLs. They are in the order
Package Manager lists them, which is also the order it searches them for a
package.

Hidden repositories are left out, add `--all` to list them as well, with a
`hidden` column. `--type` keeps only one type of repository, e.g. `R`,
`Bioconductor` or `Python`.

The `auth` column is only filled in by Package Manager versions that report
it. To use a repository that needs a token, log in with [`rig auth
login`](auth.qmd#rig-auth-login) or set `PACKAGEMANAGER_TOKEN`, see the
[authentication guide](../auth.qmd).

## Examples

```sh
rig ppm repos
rig ppm repos --type R --all
```
