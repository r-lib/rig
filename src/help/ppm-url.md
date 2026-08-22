Print the Posit Package Manager URL

## Description

Print the base URL of the Posit Package Manager instance the other
[`rig ppm`](ppm.qmd) commands report on, and nothing else, so it can be
used directly in a script:

```sh
curl "$(rig ppm url)/__api__/repos"
```

This is `https://packagemanager.posit.co` unless the
`PACKAGEMANAGER_ADDRESS` environment variable is set, in which case it is
that, with any trailing slash removed.

The `RIG_PPM_STATUS_URL` environment variable overrides the URL of the
status document alone, and takes precedence over `PACKAGEMANAGER_ADDRESS`
for that one document. If you set it, this command still prints the base
URL, while [`rig ppm status`](ppm.qmd#rig-ppm-status) reports on the
instance `RIG_PPM_STATUS_URL` names and shows which URL that was.
