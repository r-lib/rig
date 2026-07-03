Solve project dependencies

## Description

Resolve the dependencies of an R project to a concrete set of package
versions.

rig reads the project manifest (e.g. `DESCRIPTION`; override with
`--input`) and uses its built-in solver to find a compatible set of
package versions from the configured repositories, without running R.

Use `--r-version` to solve for a specific R version, `--dev` to include
development dependencies, and `--renv` to write the result as an
`renv.lock` file.
