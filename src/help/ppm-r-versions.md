List the R versions Posit Package Manager builds for

## Description

List the minor R versions Posit Package Manager builds binary packages for,
newest first, one per line and without a header, so the output can be
piped straight into other commands.

These are minor versions, e.g. `4.5`, because that is the granularity R
uses for binary compatibility: a package built for R 4.5 works with every
R 4.5.x.

rig reuses P3M's status document for up to a day, so this command normally
answers without contacting the server.
