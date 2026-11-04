Print the password or token rig uses for a URL

## Description

The argument is a URL, or the name of a configured repository, as for
[`rig auth login`](#rig-auth-login).

Print the password or token that rig sends to a URL, e.g. to use it in a
script. Include the username in the URL, or use `--username`, if the
credentials are in the system credential store.

rig prints only the secret, so be careful where the output goes.

## Examples

```sh
curl -u "__token__:$(rig auth token https://ppm.example.com)" \
  https://ppm.example.com/__api__/status
```
