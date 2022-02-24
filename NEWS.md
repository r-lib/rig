
# rim 0.1.6 (unreleased)

* On macOS rim now asks for your password for tasks that require admin
  access.

* On Windows rim now automatically elevates to administrator privileges
  as needed, by re-running with gsudo. gsudo is now bundled in the
  Windows distribution.

* New `rim system clean-registry` command to remove leftover
  registry entries.

* `rim rm` now cleans the registry on Windows.

# rim 0.1.5

* Experimental Windows version.

# rim 0.1.4

* `resolve` and `add` work again.

# rim 0.1.3

* `bash` and `zsh` completions out of the box, on macOS

* You can now pass URLs `.pkg` installers to `rim add` on macOS.

# rim 0.1.2

* `rim ls` is now a synonym of `rim list`.

* We have macOS installers now, and they are signed and notarized.

# rim 0.1.1

* `rim rm now supports removing multiple versions.

* `rim system` commands `create-lib`, `fix-permissions` and
  `make-orthogonal` can be now restricted to one or more R versions.
  `rim add` now only calls them for the newly installed R version.

* `rim system add-pak` is now implemented.

# rim 0.1.0

First pre-release.
