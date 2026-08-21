List the platforms Posit Package Manager builds for

## Description

List the build targets Posit Package Manager currently offers.

P3M also reports the targets it has retired — on the public instance those
are more than half of the list — and none of them is something to build
against today, so they are left out. Add `--all` to list them too, with a
`hidden` column marking which is which. `--all` applies to `--json` as
well.

Apart from that the list is as P3M reports it: targets it serves but does
not build binaries for are included, and the several entries that share one
`platform` are not merged.

## The columns

* `name` — P3M's name for the entry.

* `os` — `linux`, `macos` or `windows`.

* `platform` — the name this target goes by in a package's build index, so
  this is the value to match against the `platform` column of
  [`rig ppm builds`](ppm.qmd#rig-ppm-builds). Several entries can share
  one: CentOS 7 and RHEL 7 both use the `centos7` binaries.

* `distribution` and `release` — the distribution P3M *builds* the target
  on, which is not always the one it serves. The `rhel9` target is built on
  Rocky Linux, so its `distribution` is `rockylinux`.

* `arch` — the architectures this target is built for. Most Linux targets
  are x86_64 only.

* `binaries` — whether P3M builds binary packages for the target at all.
  Where this is off, the target is still served, from source.

* `hidden` — only shown with `--all`: set on the targets P3M no longer
  advertises, i.e. retired distribution releases. Their binaries stay
  downloadable, which is why they can be listed at all.

The `manylinux_2_28` target is the generic glibc build P3M serves to any
Linux it has no specific target for. It appears under the distribution it
happens to be built on, not the ones it serves.

rig reuses P3M's status document for up to a day, so this command normally
answers without contacting the server.
