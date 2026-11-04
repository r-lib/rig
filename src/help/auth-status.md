Show which repositories have credentials

## Description

Show, for every repository of the default R installation, and for the
repositories added with [`rig repos add`](repos.qmd#rig-repos-add), whether rig found credentials for
it, and where. With a URL or a repository name, show only that.

The `source` column tells where the credentials come from:

* `url`: a password in the URL.
* `env:RIG_REPO_<NAME>_PASSWORD`: an environment variable.
* `ppm-sso:<file>`: a PPM SSO token.
* `netrc:<file>`: a netrc file.
* `keyring:<backend>:<key>`: the system credential store.

If rig found no credentials, this column says why, if it knows.

rig then lists the PPM SSO tokens, with their expiry times. With `--connect`,
rig also asks each PPM instance whether it accepts the token.

This command never prints passwords or tokens.

## Examples

```sh
rig auth status
rig auth status https://alice@cran.example.com/cran
rig auth status --connect
```
