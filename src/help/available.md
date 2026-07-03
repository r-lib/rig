## Description

List R versions available to install.

By default some releases are omitted from the output:

- Versions older than R 3.0.0 are omitted. The installation of these
  might not work at all.
- Only the latest release is shown for each minor version. E.g.
  R 4.2.3 is listed, but other R 4.2.x versions are not.
  Use `--all` to list all versions.

Use `--json` to return the output in JSON. JSON output includes the
full time stamp (if available) and the download URL as well.

With the `--list-distros` flag it lists supported Linux distributions.

With the `--list-rtools-versions` flag it lists supported Rtools versions.
Rtools contains tools to build R and R packages on Windows.
Use `--all` to list all Rtools versions, even very old ones.
