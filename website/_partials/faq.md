<details>
<summary>Why does rig create a user package library?</summary>
>
>Installing non-base packages into a user package library has several
> benefits:
>
> - The system library is not writeable for regular users on some systems
>   (Windows and Linux, typically), so we might as well create a
>   properly versioned user library at the default place.
> - Some tools need a clean R environment, with base packages only, and do
>   not work well if user packages are installed into the system library.
>   E.g. `R CMD check` is such a tool, and https://github.com/r-lib/revdepcheck
>   is another.
> - You can delete an R installation (e.g. with `rig rm`) and then
>   install it again, without losing your R packages.
>
</details>

<details>
<summary>Why does rig install pak?</summary>
>
> To be able to install R packages efficiently, from CRAN, Bioconductor or
> GitHub, right from the start. pak also supports installing system libraries
> automatically on some Linux systems.
>
> If you don't want `rig add` to install pak, use the `--without-pak` option.
</details>

<details>
<summary>Why does rig change the permissions of the system library
(on macOS)?</summary>
>
> In admin mode rig changes the permissions of the system library from
> the default user-writeable to admin-writeable. This is to make sure that
> you don't install packages accidentally into the system
> library. See "Why does rig create a user package library?" above.
>
</details>

<details>
<summary>Why does rig set the default CRAN mirror?</summary>
>
> To avoid the extra work the users need to spend on this.
>
> The <https://cloud.r-project.org> mirror is usually better than the
> others, in that it is a CDN that is close to most users, and that it is
> updated more often.
>
> If you want to use a different mirror, you can set the `repos` option
> in your `.Rprofile`, so the rig repo settings will be ignored.
>
> You can also use the `--without-repos=cran` option of `rig add`.
>
</details>

<details>
<summary>Why does rig set up P3M?</summary>
>
> P3M ([Posit Public Package Manager](https://packagemanager.posit.co/client/#/))
> is generally superior to a regular CRAN mirror on Windows and many Linux
> systems.
>
> On Linux it includes binary packages for many popular distributions.
>
> On Windows, it includes up to date binary packages for older R versions as
> well.
>
> To avoid P3M use the `--without-repos=p3m` option of `rig add`.
>
</details>

<details>
<summary>Can rig install R without admin permissions</summary>
>
> Yes. rig has a [*user mode*](admin-vs-user-mode.qmd) that installs
> everything into your home directory, so it never needs `sudo` or
> administrator rights. In user mode rig installs R into
> `~/.local/share/rig/r` (`%APPDATA%\rig\data\r` on Windows) and creates
> startup links and quick links in `~/.local/bin`
> (`%USERPROFILE%\.local\bin` on Windows), which need to be on your `PATH`.
>
> `rig system user-mode` switches to user mode and migrates an existing
> admin-mode setup (see `rig system user-mode --help`).
>
> Use `rig config set mode=user` and `rig config set mode=admin` to switch
> between user and admin mode.
>
> Alternatively, switch to user mode temporarily by setting the
> `RIG_MODE=user` environment variable, or by passing the `--user` flag to
> any rig command.
>
> The default is still *admin mode*, which installs R system-wide and
> needs `sudo` (or an administrator account on Windows). Admin mode is
> also the older and better tested mode, especially on Linux, where user
> mode installs portable (manylinux or musl) R builds and uses manylinux
> R packages, both of which are newer and less widely used than the
> distro-specific builds and packages of admin mode. If you do have an
> administrator account, and no reason to keep R in your home directory,
> stay in admin mode for now.
>
</details>

<details>
<summary>How is rig different from RSwitch?</summary>
>
> While there is a small overlap in functionality, rig and
> [RSwitch](https://rud.is/rswitch/) are very different.
> I suggest you look over the features of both to decide which one suits
> your needs better.
>
> If you run rig in admin mode and also like the extra features of RSwitch,
> then you can use them together just fine: changing the default R version
> in RSwitch also changes it in rig and vice versa. You can use the rig
> cli and the RSwitch app together, or you can also use both menu bar apps
> at the same time.
>
> However, you can't use RSwitch with rig if you use rig in user mode,
> because RSwitch only works with admin mode R installations.
>
</details>

<details>
<summary>Why does rig install fonts on Linux?</summary>
>
> The portable R builds rig installs on Linux bundle the fontconfig
> library, but not its configuration and not any fonts. On a minimal
> system — a slim container, Alpine, a bare server — R then has nothing to
> render text with, and crashes on the first plot.
>
> So after installing a portable build, `rig add` writes a `fonts.conf`
> and downloads a small subset of the DejaVu fonts next to the R
> installations, and sets `FONTCONFIG_FILE` in the R installation's base
> `Rprofile` to point at the configuration. Run `rig system dirs --fonts`
> to see where they are.
>
> The configuration also lists the standard system font directories
> (`/usr/share/fonts`, `~/.local/share/fonts`, ...), so your own fonts
> keep working. If you only want the configuration and not the extra
> fonts, use `rig add --without-fonts`. If you set `FONTCONFIG_FILE`
> yourself, rig's setting is ignored.
>
</details>

<details>
<summary>How do I install my own fonts, without administrator rights?</summary>
>
> You never need to touch rig's font configuration for this. On all three
> platforms there is a per-user font directory that R (and rig's font
> configuration on Linux) already looks at, and installing a font is just a
> matter of copying the font file there. Do *not* edit the `fonts.conf`
> files that rig writes: rig rewrites them on every install, so your
> changes would be lost.
>
> **Linux.** Copy the font files into `~/.local/share/fonts`
> (or `~/.fonts`):
>
> ```sh
> mkdir -p ~/.local/share/fonts
> cp MyFont-*.ttf ~/.local/share/fonts/
> ```
>
> The `fonts.conf` that rig writes for the portable R builds lists both
> directories, so every R version rig installed picks the new fonts up.
> You do not need to run `fc-cache`: fontconfig notices that the font
> directory is newer than its cache and rescans it at startup. (You *can*
> run the host's `fc-cache` if it has one, but it might write a cache in a
> different format version than the fontconfig bundled in the R build,
> in which case it has no effect.)
>
> Do not put your own fonts into rig's own font directory
> (`rig system dirs --fonts`): rig deletes and re-extracts that directory
> when it re-downloads the fallback fonts.
>
> **macOS.** Copy the font files into `~/Library/Fonts`, or open them in
> Font Book and install them "for this user only":
>
> ```sh
> cp MyFont-*.ttf ~/Library/Fonts/
> ```
>
> This works for the quartz devices (which use CoreText, and see
> `~/Library/Fonts` automatically) and for the cairo and X11 devices: the
> `fonts.conf` in the R installation lists `~/Library/Fonts`,
> `~/.local/share/fonts` and `~/.fonts`. If a new font does not show up in
> a cairo device, refresh the fontconfig cache with the `fc-cache` in the
> R installation, which knows where R's configuration is:
>
> ```sh
> "$(R RHOME)/bin/fc-cache" -f
> ```
>
> **Windows.** R uses the system font list on Windows, not fontconfig, and
> rig does not configure fonts at all there. To install a font for
> yourself only, right-click the font file in Explorer and choose
> *Install for current user* (or open it and click *Install* — Windows
> installs it into `%LOCALAPPDATA%\Microsoft\Windows\Fonts` for your user,
> without administrator rights).
>
> Then map it to an R font family before using it in the `windows()`
> device, using the font's real family name:
>
> ```r
> windowsFonts(myfont = windowsFont("My Font"))
> plot(1, family = "myfont")
> ```
>
> The [ragg](https://ragg.r-lib.org/) and
> [systemfonts](https://systemfonts.r-lib.org/) packages find per-user
> fonts without this extra step on all three platforms; use
> `systemfonts::system_fonts()` to check that a font was installed
> correctly.
>
</details>

<details>
<summary>Which domains does rig download files from?</summary>
>
> Here is the list of domains that you need to enable in your proxy.
> Note that some of these, in particular the GitHub ones,  might
> trigger redirects.
>
> - https://api.r-hub.io/rversions for resolving R versions, i.e. this is
>   needed for `rig install`, `rig available`, etc.
> - `rig install` downloads pak from https://r-lib.github.io/p/pak
>   unless requested otherwise.
> - `rig install` sets https://cloud.r-project.org as the default CRAN
>   mirror, unless requested otherwise.
> - `rig install` sets https://packagemanager.posit.co as the Posit
>   Package Manager CRAN mirror on supported Linux systems, unless
>   requested otherwise.
> - `rig install` downloads the EPEL package from
>   https://dl.fedoraproject.org/pub/epel on RHEL systems.
> - `rig rtools` downloads Rtools from the following URLs on Windows:
>   * https://github.com/r-hub/rtools44/releases,
>   * https://github.com/r-hub/rtools43/releases,
>   * https://github.com/r-hub/rtools42/releases,
>   * https://cloud.r-project.org/bin/windows/Rtools
> - https://github.com/R-macos/gcc-darwin-arm64/releases,
>   https://github.com/fxcoudert/gfortran-for-macOS/releases and whatever
>   domains Homebrew is using, to download system packages for
>   `rig sysreqs` on macOS.
> - `rig add` downloads https://curl.se/ca/cacert.pem (the CA bundle) and
>   the fallback fonts from https://github.com/r-lib/rig/releases for the
>   portable Linux R builds.
</details>
