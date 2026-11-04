Save credentials for a repository or Package Manager instance

## Description

The argument is a URL, or the name of a repository of the default R
installation or one added with [`rig repos add`](repos.qmd#rig-repos-add). For a name, rig uses the URL
of that repository, including the username in it, if there is one.

Without `--username`, log in to a Posit Package Manager (PPM) instance with
single sign-on (SSO). rig prints a URL and a code, and opens the URL in
your browser. After you log in there, rig saves the PPM token in
`~/.ppm/tokens.toml` (or the file `PACKAGEMANAGER_SSO_TOKEN_FILE` names), the
same file pak uses. The URL is the base URL of the PPM instance, by default
the value of the `PACKAGEMANAGER_ADDRESS` environment variable.

If `PACKAGEMANAGER_IDENTITY_TOKEN_FILE` is set, e.g. on a CI system, rig
exchanges the identity token in that file for a PPM token, instead of using
the browser.

With `--username`, save a password for a repository in the system credential
store: macOS Keychain, Windows Credential Manager, or a Secret Service
(e.g. GNOME Keyring) on Linux. rig asks for the password, or reads it from
the standard input with `--password -`. rig does not check the password.

rig only looks in the credential store for repository URLs that include the
username, so add the repository with the username in its URL, e.g.
`https://alice@cran.example.com/cran`. The URL you log in to can be a
repository URL, or the URL of the host, e.g. `https://cran.example.com`, to
use the same password for every repository there. It does not have to be a
configured repository; if none uses the credentials yet, rig tells you.

See the [authentication guide](../auth.qmd) for details.

## Examples

```sh
# Log in to your PPM instance with SSO
export PACKAGEMANAGER_ADDRESS=https://ppm.example.com
rig auth login

# Save a password for a repository
rig auth login https://cran.example.com/cran --username alice

# The same, for the repository you added as `acme`, with
# `rig repos add acme https://alice@cran.example.com/cran`
rig auth login acme

# Read the password from the standard input
echo "$PASSWORD" | rig auth login https://cran.example.com/cran --username alice --password -
```
