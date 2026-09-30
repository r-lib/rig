Install new Rtools version [alias: install]

## Description

Install new Rtools versions on Windows.

You can specify the new version(s) by their name, optionally with an
'rtools' prefix, e.g. '43' or 'rtools43'.

If `version` is 'all' (the default), then all Rtools versions that are
required for the currently installed R versions will be installed.

In user mode (`RIG_MODE=user`) Rtools is installed per-user, without
administrator rights, into `%APPDATA%\rig\data\rtools` (override with the
`RIG_RTOOLS_INSTALL_DIR` environment variable or the `rtools-install-dir`
config setting). rig points each R version at it by setting
`RTOOLS<ver>_HOME` in that R version's `etc\Renviron.site` and
`etc\Rcmd_environ`.

In admin mode this command needs an administrator account.

With `--json`, rig prints a JSON array to the standard output, with one
entry for each Rtools version it installed or found already installed. The
fields are the same as in `rig rtools list --json` (`name`, `version`,
`fullversion`, `path`, `arch`), plus `new-install`, which is `true` if rig
installed that Rtools version now and `false` if it was already installed.
All other messages go to the standard error.

## Examples

```sh
rig rtools add 43
rig rtools add all
rig rtools add --json
```
