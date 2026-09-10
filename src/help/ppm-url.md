Print the Posit Package Manager URL

## Description

Print the base URL of the Posit Package Manager instance the other [`rig ppm`](ppm.qmd)
commands report on, and nothing else, so it can be used directly in a
script:

```sh
curl "$(rig ppm url)/__api__/repos"
```

This is `https://packagemanager.posit.co` unless the `PACKAGEMANAGER_ADDRESS`
environment variable is set, in which case it is that, with any trailing
slash removed.
