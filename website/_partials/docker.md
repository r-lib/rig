Use the `ghcr.io/r-lib/rig/r` Docker container to easily run multiple
R versions.
It is currently based on Ubuntu 22.04 and contains rig and the six latest
R versions, including R-next and R-devel.
It is available for x86_64 and arm64 systems:

```
> docker run ghcr.io/r-lib/rig/r rig ls
* name   version    aliases
------------------------------------------
  4.1.3
  4.2.3
  4.3.3
  4.4.3             oldrel
* 4.5.1             release
  devel  (R 4.6.0)
  next   (R 4.5.1)
```

See this image on
[GitHub](https://github.com/r-lib/rig/pkgs/container/rig%2Fr).

## All containers

We also have other containers with rig and either R-devel and R-release
preinstalled, on various Linux distros.
Here is a table of all containers:

Name                                      | OS                 | R version      | Tags
------------------------------------------|--------------------|----------------|------------------------------------------------------------------------------------------------
`ghcr.io/r-lib/rig/ubuntu-24.04-multi`    | Ubuntu 24.04       | last 6 (daily) | `r`, `rig`, `multi`, `ubuntu-multi`
`ghcr.io/r-lib/rig/ubuntu-26.04-release`  | Ubuntu 26.04       | release        | `release`, `ubuntu`, `ubuntu-release`, `ubuntu-latest`, `ubuntu-latest-release`, `ubuntu-26.04`
`ghcr.io/r-lib/rig/ubuntu-26.04-devel`    | Ubuntu 26.04       | devel (daily)  | `devel`, `ubuntu-devel`, `ubuntu-latest-devel`
`ghcr.io/r-lib/rig/ubuntu-24.04-release`  | Ubuntu 24.04       | release        | `ubuntu-24.04`
`ghcr.io/r-lib/rig/ubuntu-24.04-devel`    | Ubuntu 24.04       | devel (daily)  |
`ghcr.io/r-lib/rig/ubuntu-22.04-release`  | Ubuntu 22.04       | release        | `ubuntu-22.04`
`ghcr.io/r-lib/rig/ubuntu-22.04-devel`    | Ubuntu 22.04       | devel (daily)  |
`ghcr.io/r-lib/rig/ubuntu-20.04-release`  | Ubuntu 20.04       | release        | `ubuntu-20.04`
`ghcr.io/r-lib/rig/ubuntu-20.04-devel`    | Ubuntu 20.04       | devel (daily)  |
`ghcr.io/r-lib/rig/debian-13-release`     | Debian 13          | release        | `debian`, `debian-release`, `debian-latest`, `debian-latest-release`, `debian-13`
`ghcr.io/r-lib/rig/debian-13-devel`       | Debian 13          | devel (daily)  | `debian-devel`, `debian-latest-devel`
`ghcr.io/r-lib/rig/debian-12-release`     | Debian 12          | release        | `debian-12`
`ghcr.io/r-lib/rig/debian-12-devel`       | Debian 12          | devel (daily)  |
`ghcr.io/r-lib/rig/fedora-43-release`     | Fedora 43          | release        | `fedora`, `fedora-release`, `fedora-latest`, `fedora-latest-release`, `fedora-43`
`ghcr.io/r-lib/rig/fedora-43-devel`       | Fedora 43          | devel          | `fedora-devel`, `fedora-latest-devel`
`ghcr.io/r-lib/rig/fedora-42-release`     | Fedora 42          | release        | `fedora-42`
`ghcr.io/r-lib/rig/fedora-42-devel`       | Fedora 42          | devel          |
`ghcr.io/r-lib/rig/opensuse-16.0-release` | OpenSUSE Leap 16.0 | release        | `opensuse`, `opensuse-release`, `opensuse-latest`, `opensuse-latest-release`, `opensuse-16.0`
`ghcr.io/r-lib/rig/opensuse-16.0-devel`   | OpenSUSE Leap 16.0 | devel (daily)  | `opensuse-devel`, `opensuse-latest-devel`
`ghcr.io/r-lib/rig/opensuse-15.6-release` | OpenSUSE Leap 15.6 | release        | `opensuse-15.6`
`ghcr.io/r-lib/rig/opensuse-15.6-devel`   | OpenSUSE Leap 15.6 | devel (daily)  |

For convenience, we also create these tags:

Tag                                | Current Image           | Description
-----------------------------------|-------------------------|------------------------------------
`ghcr.io/r-lib/rig/r`              | `ubuntu-24.04-multi`    | Last 6 R versions on latest Ubuntu.
`ghcr.io/r-lib/rig/rig`            | "                       | "
`ghcr.io/r-lib/rig/multi`          | "                       | "
`ghcr.io/r-lib/rig/ubuntu-multi`   | "                       | "
`ghcr.io/r-lib/rig/release`        | `ubuntu-26.04-release`  | Latest R release.
`ghcr.io/r-lib/rig/ubuntu`         | `ubuntu-26.04-release`  | Latest R release on latest Ubuntu.
`ghcr.io/r-lib/rig/ubuntu-26.04`   | `ubuntu-26.04-release`  | Latest R release on Ubuntu 26.04.
`ghcr.io/r-lib/rig/devel`          | `ubuntu-26.04-devel`    | R devel.
`ghcr.io/r-lib/rig/ubuntu-devel`   | `ubuntu-26.04-devel`    | R devel on latest Ubuntu.
`ghcr.io/r-lib/rig/ubuntu-24.04`   | `ubuntu-24.04-release`  | Latest R release on Ubuntu 24.04.
`ghcr.io/r-lib/rig/ubuntu-22.04`   | `ubuntu-22.04-release`  | Latest R release on Ubuntu 22.04.
`ghcr.io/r-lib/rig/ubuntu-20.04`   | `ubuntu-20.04-release`  | Latest R release on Ubuntu 20.04.
`ghcr.io/r-lib/rig/debian`         | `debian-13-release`     | Latest R release on latest Debian.
`ghcr.io/r-lib/rig/debian-13`      | `debian-13-release`     | Latest R release on Debian 13.
`ghcr.io/r-lib/rig/debian-12`      | `debian-12-release`     | Latest R release on Debian 12.
`ghcr.io/r-lib/rig/debian-devel`   | `debian-13-devel`       | R devel on latest Debian.
`ghcr.io/r-lib/rig/fedora`         | `fedora-43-release`     | Latest R release on latest Fedora.
`ghcr.io/r-lib/rig/fedora-43`      | `fedora-43-release`     | Latest R release on Fedora 43.
`ghcr.io/r-lib/rig/fedora-42`      | `fedora-42-release`     | Latest R release on Fedora 42.
`ghcr.io/r-lib/rig/fedora-devel`   | `fedora-43-devel`       | R devel on latest Fedora.
`ghcr.io/r-lib/rig/opensuse`       | `opensuse-16.0-release` | Latest R release on latest OpenSUSE.
`ghcr.io/r-lib/rig/opensuse-16.0`  | `opensuse-16.0-release` | Latest R release on OpenSUSE 16.0.
`ghcr.io/r-lib/rig/opensuse-15.6`  | `opensuse-15.6-release` | Latest R release on OpenSUSE 15.6.
`ghcr.io/r-lib/rig/opensuse-devel` | `opensuse-16.0-devel`   | R devel on latest OpenSUSE.

See all container images on
[GitHub](https://github.com/orgs/r-lib/packages?repo_name=rig).

## Docker container features

For all containers:

* rig is pre-installed, so you can easily add or remove R versions.
* https://github.com/r-lib/pak is installed for all R versions.
* Automatic system dependency installation via pak.
* Linux binary packages are automatically installed from the
  [Posit Public Package Manager](https://packagemanager.posit.co/client/#/)
  in x86_64 containers, on Ubuntu, Debian and OpenSUSE.
* Available on x86_64 and aarch64.
