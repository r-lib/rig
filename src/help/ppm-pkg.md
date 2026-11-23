Query and publish packages in Posit Package Manager

## Description

Commands about the packages of a Package Manager repository: list and
search them, show one package and its version history, check which builds
exist, show build logs, vulnerabilities and download counts, and
publish a package.

`--repo` selects a repository, see [`rig ppm repos`](#rig-ppm-repos) for
the repositories of the instance. Without it, `info` and `history` use the
first repository that has the package, `list` and `vulns` use all R and
Bioconductor repositories.

`publish` needs a token. A private instance may need one for every command,
see [`rig ppm`](#rig-ppm).
