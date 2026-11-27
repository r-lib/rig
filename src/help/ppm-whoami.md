Show your identity and token for Package Manager

## Description

Show who the Package Manager instance thinks you are, and which token rig
sends to its API: where the token is from, its subject, scopes and expiry
time, and whether the server accepts it.

Without a token you are an anonymous user. See the [authentication guide](../auth.qmd) for
how to get a token, e.g. with [`rig auth login`](auth.qmd#rig-auth-login) or the `PACKAGEMANAGER_TOKEN`
environment variable.

## Examples

```sh
rig ppm whoami
```
