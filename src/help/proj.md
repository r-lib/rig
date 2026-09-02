Manage R projects (experimental)

## Description

Manage R projects (experimental).

A project is a directory with a package manifest, typically a
`DESCRIPTION` file, that declares the R packages the project depends on.
`rig proj` resolves those dependencies against the configured package
repositories and can install them into a project library.

`rig proj deps` shows the direct and recursive dependencies of the
project.
`rig proj tree` shows the recursive dependencies as a tree, so you can
see how each package is pulled in.
`rig proj lock` resolves the full dependency tree to a concrete set of
package versions, writes the result to `rproj.lock`, and can also write
an `renv.lock` file.
`rig proj sync` installs the dependencies `rproj.lock` resolved into a
package library.

Dependencies are resolved with rig's built-in solver, so R does not need
to be running for `rig proj deps`, `rig proj tree` and `rig proj lock`.

`rig proj` is currently experimental, and might change in future
versions. Feedback is appreciated.
