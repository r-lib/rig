Remove saved credentials

## Description

The argument is a URL, or the name of a configured repository, as for
[`rig auth login`](#rig-auth-login).

Without `--username`, remove the PPM SSO token of a Posit Package Manager
instance from the token file. The URL is the base URL of the instance, by
default the value of `PACKAGEMANAGER_ADDRESS`.

With `--username`, remove the password of that user for the URL from the
system credential store.

This only removes credentials that rig (or pak) saved on this computer. It
does not end your session on the server.

## Examples

```sh
rig auth logout
rig auth logout https://cran.example.com/cran --username alice
```
