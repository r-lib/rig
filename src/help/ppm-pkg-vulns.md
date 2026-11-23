List known vulnerabilities of packages

## Description

List the known vulnerabilities of packages, from Package Manager's
vulnerability database. Give package names to only show those packages.

By default rig queries every R and Bioconductor repository of the instance,
see [`rig ppm repos`](#rig-ppm-repos). Repositories without vulnerability
data, e.g. ones with Git sources, are skipped. `--repo` queries a single
repository.

The `fixed` column shows the versions that fix the vulnerability. The
`repos` column shows which of the queried repositories have the vulnerable
package. `--json`
prints the full records, in the OSV format, per repository.

## Examples

```sh
rig ppm pkg vulns
rig ppm pkg vulns haven readxl
rig ppm pkg vulns --repo cran --json
```
