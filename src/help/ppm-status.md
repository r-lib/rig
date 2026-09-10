Show the Posit Package Manager status report

## Description

Show the full status report of a Posit Package Manager instance: what it is
and what it is configured for, then the R versions, build targets,
Bioconductor releases and macOS build flavors it serves.

## The report

The report shows an initial block and then four tables:

* **R versions** — the minor R versions binaries are built for. Same list
  as [`rig ppm r-versions`](ppm.qmd#rig-ppm-r-versions) .

* **Build targets** — the platforms, as reported, retired ones included.
  Same table as [`rig ppm platforms --all`](ppm.qmd#rig-ppm-platforms) , which documents the columns.

* **Bioconductor versions** — each Bioconductor release, the R version it
  goes with, and the CRAN snapshot it is pinned to.

* **macOS binaries** — the macOS build flavor used for each R version, per
  architecture. `default` is what a newer R version gets. An empty cell means
  there are no macOS binaries for that R version and architecture.

An instance may report fields rig does not show. Use `--json` to see the
status document in full; nothing is dropped from it.

This command does not cache information from PPM, it always performs an
HTTP query.

## Which server

`https://packagemanager.posit.co` by default, or the instance
`PACKAGEMANAGER_ADDRESS` names. `RIG_PPM_STATUS_URL` overrides the URL of the
status document alone and wins over `PACKAGEMANAGER_ADDRESS`; the report's
first line shows the URL actually used.

## Examples

```sh
# The public instance
rig ppm status

# Your own instance
PACKAGEMANAGER_ADDRESS=https://ppm.example.com rig ppm status

# The complete status document, including fields rig does not display
rig ppm status --json
```
