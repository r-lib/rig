Manage R projects (experimental)

## Description

Manage R projects (experimental).

A project is a directory with a package manifest, typically a
`DESCRIPTION` file, that declares the R packages the project depends on.
`rig proj` resolves those dependencies against the configured package
repositories and can install them into a project library.

`rig proj deps` shows the direct and recursive dependencies of the
project.
`rig proj solve` resolves the full dependency tree to a concrete set of
package versions, and can write the result to an `renv.lock` file.
`rig proj deploy` installs the resolved dependencies into a package
library.

Dependencies are resolved with rig's built-in solver, so R does not need
to be running for `rig proj deps` and `rig proj solve`.

`rig proj` is currently experimental, and might change in future
versions. Feedback is appreciated.
