Show the version history of a package

## Description

Show which version of a package was current in which snapshot of a
repository, newest first. Each row is a snapshot in which the package
changed.

Without `--repo`, rig uses the first repository that has the package.
For a Bioconductor repository rig uses the newest Bioconductor version,
select another one with `--bioc-version`.

The snapshot dates can be used with [`rig ppm url
--snapshot`](#rig-ppm-url) to get a repository URL with that version of the
package.

## Examples

```sh
rig ppm pkg history dplyr
rig ppm pkg history dplyr --limit 5 --json
```
