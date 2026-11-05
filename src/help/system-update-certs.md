Find or download the CA certificate bundle and configure R to use it

## Description

Find or download the CA certificate bundle and configure R to use it
(Linux only, [user mode](../admin-vs-user-mode.qmd)).

rig points every installed R version at the system CA bundle, e.g.
`/etc/ssl/certs/ca-certificates.crt`, so R trusts the same certificates as
the rest of the system. If there is no system bundle, rig downloads the
Mozilla bundle from curl.se instead.

Run this after adding a certificate to the system store, e.g. with
`update-ca-certificates`, if your R versions still use the downloaded
bundle. It also helps on systems where the downloaded bundle is out of
date.

`SSL_CERT_FILE` and `CURL_CA_BUNDLE`, if they are set when R starts, take
precedence over the bundle that rig configures.
