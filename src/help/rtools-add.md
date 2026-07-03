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
config setting). rig points each R version at it by setting `RTOOLS<ver>_HOME`
in that R version's `etc\Renviron.site`.

In admin mode this command needs an administrator account.

## Examples

```sh
rig rtools add 43
rig rtools add all
```
