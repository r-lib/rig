List the platforms Posit Package Manager builds for

## Description

List the build targets Posit Package Manager currently offers.

Add `--all` to list retired targets as well, with a `hidden` column marking
which is which. `--all` applies to `--json` as well.

## The columns

* `name` — PPM's name for the entry.

* `os` — `linux`, `macos` or `windows`.

* `platform` — the name this target goes by in a package's build index, so
  this is the value to match against the `platform` column of
  [`rig ppm builds`](ppm.qmd#rig-ppm-builds). Several entries can share
  one: CentOS 7 and RHEL 7 both use the `centos7` binaries.

* `distribution` and `release` — the distribution PPM *builds* the target
  on, which is not always the one it serves. The `rhel9` target is built on
  Rocky Linux, so its `distribution` is `rockylinux`.

* `arch` — the architectures this target is built for. Most Linux targets
  are x86_64 only.

* `binaries` — whether PPM builds binary packages for the target at all.
  Where this is off, the target is still served, from source.

* `hidden` — only shown with `--all`: set on the targets PPM no longer
  advertises, i.e. retired distribution releases. Their binaries stay
  downloadable, which is why they can be listed at all.
