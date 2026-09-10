Packages installed in a library

## Description

List the packages installed in an R package library. This command does not
start R.

Example output:
```
312 packages (R 4.4.1, main: /Users/gaborcsardi/Library/R/arm64/4.4/library)

Package     Version      Built   Platform                 Source
-----------------------------------------------------------------------------
cli         3.6.3        4.4.0   aarch64-apple-darwin20   CRAN
glue        1.8.0        4.4.1   aarch64-apple-darwin20   CRAN
asciicast   2.3.1.9000   4.4.1   aarch64-apple-darwin20   github::r-lib/asciicast
mypkg       0.0.1        4.4.1   -                        -
```

`Platform` is `-` for a package without compiled code.
Use `--json` for machine readable output, which reports the repository or
remote type as `source` and the remote itself as `remote`, separately,
instead of `Source`.

`--library` (`-l`) selects a non-default library. It takes either the name
of a library of the R version, see [`rig library list`](library.qmd), or
the path of a library directory.

`--r-version` (`-r`) lists the library of another R version, instead of the
default one.
