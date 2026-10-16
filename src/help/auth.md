Manage credentials for repositories and Package Manager instances

## Description

rig can use HTTP basic authentication when downloading packages, metadata
or when using Posit Package Manager's API.

These are the rules for when rig sends an `Authorization` header and how it
looks up credentials:

1. A password in the URL. This is only allowed for URLs given on the
   command line, with `--with-repos`, and rig does not store it.
2. The `RIG_REPO_<NAME>_USERNAME` and `RIG_REPO_<NAME>_PASSWORD` environment
   variables, for the repository called `<NAME>`: the name you gave to
   [`rig repos add`](repos.qmd#rig-repos-add), in `--with-repos <name>=<url>`, or in `[[repository]]` in
   `rproj.toml`. [`rig auth status`](#rig-auth-status) shows it in the `name` column. `<NAME>` is in
   upper case, with every character that is not a letter or a digit
   replaced by `_`.
3. A PPM single sign-on (SSO) token from [`rig auth login`](#rig-auth-login), or from pak.
4. A netrc file.
5. The system credential store (macOS Keychain, Windows Credential Manager,
   Secret Service on Linux), only for URLs that include a username.
   Passwords saved with [`rig auth login --username`](#rig-auth-login) go here.

rig uses the same netrc file, the same credential store keys and the same
PPM token file as pak, so credentials that work with pak also work with
rig, and the other way around.

These credentials are only used by rig itself. R's own `install.packages()`
does not use them.

rig never stores passwords in its configuration file, in `rproj.toml` or in
`rproj.lock`.

See the [authentication guide](../auth.qmd) for details and examples.
