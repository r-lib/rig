---
name: bump-distro
description: >-
  Add support for a new Linux distro version (e.g. "Fedora 44", "Ubuntu 26.04",
  "OpenSUSE 16.0", "Debian 13") or retire an old one across rig's docs, package
  test matrix, and container image matrix. Use when the user says things like
  "add Fedora 44 support", "add a new Ubuntu container", "retire Fedora 42", or
  "drop Debian 11". Handles both the ADD and RETIRE operations for every distro
  family (Fedora, Ubuntu, Debian, OpenSUSE) and knows the file-by-file edits,
  the matrix.json alias/tag demote pattern, and the README.md hand-edit caveat.
---

# Bumping a supported Linux distro version

rig declares distro support in several **unlinked** places. Miss one and the
docs, the package test matrix, or the container images drift out of sync. This
skill is the checklist. There are two mirror-image operations: **ADD** a new
version and **RETIRE** an old one.

Precedent lives in git — the most reliable guide is always the last analogous
commit. Search first:

```bash
git log --oneline -- containers/matrix.json        # container add/retire
git log --oneline --grep -iE 'fedora|ubuntu|debian|suse|retire|add.*container'
```

Recent examples: `8acb7bf` (add Fedora 43 container), `2556fd9` (retire Fedora
42 docs), `5af5c9e` (retire Fedora 42 container).

## Prerequisite for ADD

The upstream **R builds must already exist** for the new version (Posit
[R-builds project](https://github.com/rstudio/r-builds)). rig only wraps them.
Confirm before starting, and confirm the base image tag exists
(`fedora:44`, `ubuntu:26.04`, …) since `containers/<dir>/Dockerfile` does
`FROM <base>:${RELEASE}`.

## Files to edit

| File | ADD | RETIRE |
|------|-----|--------|
| `website/_partials/install.md` — supported list | add version to `- Fedora Linux …` line | remove from supported line, add `- Fedora N (last R version: X.Y.Z),` to the **Retired** `<details>` list |
| `README.md` — supported list | same as install.md | same as install.md |
| `Makefile` — `VARIANTS` | add `<name>` to the list | remove `<name>` |
| `containers/matrix.json` | add new release+devel blocks, demote old (see below) | delete the retired version's release+devel blocks |
| `website/_partials/docker.md` — 2 tables | add new-latest rows, demote old to pinned | leave old rows (images persist) or drop if fully gone |
| `README.md` — 2 docker tables | same as docker.md (README is more minimal — no old pinned rows historically) | same |

Notes:
- `README.md` and `install.md` carry the **same** supported/retired lists —
  edit both identically.
- The retired-list `(last R version: X.Y.Z)` is the *actual* last R version
  built for that distro — look it up, don't guess.
- `Makefile` `VARIANTS` uses the **package-test image names**, which differ from
  container names: `fedora-44`, `ubuntu-24.04`, `debian-13`,
  `opensuse/leap-15.6`, `rockylinux/rockylinux-9`, `redhat/ubi9`,
  `almalinux-9`. Match the existing style.

## containers/matrix.json — the alias/tag pattern

`matrix.json` is the **source of truth** (not generated; `matrix.py` only
filters it). The CI workflow `conts.yml` builds from it. Each version has a
`-release` and a `-devel` entry.

The rig registry short name `<reg>` per family: `fedora`, `ubuntu`, `debian`,
`opensuse`. The `dir` and `args` per family:

| Family | `dir` | `args` |
|--------|-------|--------|
| Fedora | `fedora` | `RELEASE=<ver>`, `RVERSION=release\|devel` |
| Ubuntu | `ubuntu` | `DISTRO=ubuntu`, `RELEASE=<ver>`, `RVERSION=…` |
| Debian | `ubuntu` | `DISTRO=debian`, `RELEASE=<ver>`, `RVERSION=…` |
| OpenSUSE | `suse` | `RELEASE=<ver>`, `RVERSION=…` |

**Aliases vs tags — the key rule:** on demote, leave `aliases` **unchanged**
(the generic ones stay; they only affect `matrix.py` selection and duplicates
are harmless). Only the **`tags`** move. The newest version owns the generic
"latest" tags; everything else keeps only its version-pinned tags.

Generic ("latest") tags the **newest** release entry owns (and a demoted one
loses):
```
ghcr.io/r-lib/rig/<reg>-latest-release:latest
ghcr.io/r-lib/rig/<reg>-latest:latest
ghcr.io/r-lib/rig/<reg>-release:latest
ghcr.io/r-lib/rig/<reg>:latest
```
Newest devel entry generic tags:
```
ghcr.io/r-lib/rig/<reg>-latest-devel:latest
ghcr.io/r-lib/rig/<reg>-devel:latest
```
Always-kept (pinned) tags on every version:
```
release: ghcr.io/r-lib/rig/<reg>-<ver>-release:latest, ghcr.io/r-lib/rig/<reg>-<ver>:latest
devel:   ghcr.io/r-lib/rig/<reg>-<ver>-devel:latest
```

**ADD steps:**
1. Demote the current-latest release entry: strip its generic tags, keeping only
   the two pinned tags. Same for its devel entry (keep the one pinned tag).
2. Insert new `<reg>-<ver>-release` and `<reg>-<ver>-devel` blocks (copy an
   existing block for the family, bump `<ver>`) with full aliases **and** full
   tags (pinned + generic).

**RETIRE steps:** delete the retired version's two blocks entirely.

### Ubuntu is special — the global default

Ubuntu is rig's default distro. Beyond the `ubuntu-*` generic tags, the newest
Ubuntu **release** entry also owns `ghcr.io/r-lib/rig/release:latest`, the
newest **devel** owns `ghcr.io/r-lib/rig/devel:latest`, and there is a separate
`ubuntu-<ver>-multi` entry owning `r:latest`, `rig:latest`, `multi:latest`,
`ubuntu-multi:latest`. Bumping Ubuntu's default therefore means moving those
global + multi tags too, plus the `release`/`devel` convenience rows in the
docker tables. Do this deliberately.

## Gotchas

- **Do NOT run `make readme`.** `README.qmd` is now ultra-minimal and no longer
  includes the supported-distros or docker sections, so regenerating would
  **delete** them. Hand-edit `README.md` directly (this is what the precedent
  commits do).
- The website partials (`install.md`, `docker.md`) are static markdown — no
  build needed; `make docs` just re-renders.
- `docker.md` documents **all images that still exist** in the registry,
  including retired/pinned older versions (that's why old rows stay). `README.md`
  historically lists only the current + one pinned — keep it minimal there.
- Container `-release` vs `-devel`: `release` = latest R release, `devel` = R
  devel (daily). Both entries always come in a pair.

## Validate

```bash
python3 -c "import json,sys; d=json.load(open('containers/matrix.json')); \
print('ok', len(d), 'entries'); \
print([x['name'] for x in d if '<reg>' in x['name']])"   # substitute family
git diff --stat
```

Cross-check that every place listing distro versions agrees:
```bash
grep -rn "<reg>-[0-9]" Makefile README.md website/_partials/*.md containers/matrix.json
```
