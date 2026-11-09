Detect the type of a package repository

## Description

Detect the type of the package repository at a URL: a CRAN mirror, a
repository of a Posit Package Manager instance (CRAN, Bioconductor, other, a
snapshot, or the root of the instance), a Bioconductor mirror or one of its
repositories, a repository of extended metadata, an r-universe repository, or
another CRAN-like repository.

rig sends a few `HEAD` requests to well-known paths below the URL, and, for
Posit Package Manager, asks its API about the repository.

This is an internal command, for testing the detection. Use `--json` to see
every field of the result.
