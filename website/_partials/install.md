Choose your operating system below to see the available install methods.

{{< include _partials/install-methods.md >}}

## Supported Linux distributions <a id="id-supported-linux-distributions"></a>

- Debian 12, 13
- Ubuntu 20.04, 22.04, 24.04, 26.04
- Fedora Linux 42, 43
- OpenSUSE 15.6, 16.0
- SUSE Linux Enterprise 15 SP6
- Red Hat Enterprise Linux 7, 8, 9, 10
- AlmaLinux 8, 9, 10
- Rocky Linux 8, 9 10

We use the R builds from the Posit
[R-builds project](https://github.com/rstudio/r-builds#r-builds).

<details><summary>Retired Linux distributions</summary>
These are not updated any more, no new R builds are added for them,
but existing R builds still work.

- CentOS 6 (only x86_64, last R version: 4.0.4),
- CentOS 7 (last R version: 4.4.3),
- CentOS 8 (last R version: 4.4.3),
- Debian 9 (last R version: 4.2.1),
- Debian 10 (last R version: 4.4.3),
- Fedora 37 (last R version: 4.3.2),
- Fedora 38 (last R version: 4.4.2),
- Fedora 39 (last R version: 4.4.3),
- Fedora 40 (last R version: 4.5.1),
- Fedora 41 (last R version: 4.5.3),
- OpenSUSE 42 (only x86_64, last R version: 4.2.1),
- OpenSUSE 15.1 (only x86_64, last R version: 4.1.2),
- OpenSUSE 15.2 (only x86_64, last R version: 4.1.3),
- OpenSUSE 15.3 (last R version: x86_64: 4.4.3, aarch64: 4.3.1),
- OpenSUSE 15.4 (last R version: 4.4.0),
- OpenSUSE 15.5 (last R version: 4.4.3),
- SUSE Linux Enterprise 15 (only x86_64, last R version: 4.1.2),
- SUSE Linux Enterprise 15.1 (only x86_64, last R version: 4.1.2),
- SUSE Linux Enterprise 15.2 (only x86_64, last R version: 4.1.3),
- SUSE Linux Enterprise 15.3 (last R version: x86_64: 4.4.3, aarch64: 4.3.1),
- SUSE Linux Enterprise 15.4 (last R version: 4.4.0),
- SUSE Linux Enterprise 15.5 (last R version: 4.4.3),
- Ubuntu 16.04 (only x86_64, last R version: 4.1.2),
- Ubuntu 18.04 (last R version: 4.3.1).
</details>

## Installing auto-complete

All rig installers and archives ship shell completion files.

### macOS and Linux

The macOS and Linux installers install completion files for `zsh` and `bash`
into system locations.

`zsh` completions work out of the box.

For `bash` completions install the `bash-completion` package from Homebrew
or your Linux distribution and make sure it is loaded from your `.bashrc`.
(You don't need to install `bash` from Homebrew, but you can if you like.)

For **user-mode** installs the completion files are placed under the install
prefix instead (e.g. `~/.local/share`):

- `zsh`: add `~/.local/share/zsh/site-functions` to your `fpath` (before
  `compinit`).
- `bash`: `bash-completion` picks up `~/.local/share/bash-completion/completions`
  automatically once it is loaded from your `.bashrc`.

### Windows

The Windows installer and archive ship a PowerShell completion script. To
enable tab-completion, dot-source it from your PowerShell profile (`$PROFILE`).
For a user-mode archive install that is:

``` powershell
. "$env:USERPROFILE\.local\share\rig\_rig.ps1"
```

```{=html}
<script>
// Open the tab matching the URL hash (#macos / #windows / #linux), e.g. when
// arriving from the "Install rig for <OS>" button on the Get started page.
(function () {
  function activateFromHash() {
    var os = (location.hash || "").replace(/^#/, "").toLowerCase();
    if (!os) return;
    var links = document.querySelectorAll("ul.nav-tabs .nav-link");
    for (var i = 0; i < links.length; i++) {
      if (links[i].textContent.trim().toLowerCase() === os) {
        links[i].click();
        // Open the tab but keep the reader at the top of the page rather than
        // scrolling down to the tabset.
        window.scrollTo(0, 0);
        break;
      }
    }
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", activateFromHash);
  } else {
    activateFromHash();
  }
  window.addEventListener("hashchange", activateFromHash);
})();
</script>
```
