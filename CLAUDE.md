# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**rig** is "The R Installation Manager" - a cross-platform CLI tool written in Rust that manages multiple R installations on macOS, Windows, and Linux. It allows users to install, remove, configure, and switch between different R versions.

## Installation Modes (user vs admin)

rig operates in one of two modes on all platforms, represented by the `Mode`
enum in `src/utils.rs`:

- **Admin mode** (the current default): R is installed system-wide and most
  operations need `sudo` / an administrator account. R goes into platform
  locations (`/opt/R` on Linux, `/Library/Frameworks/R.framework` on macOS,
  `C:\Program Files\R` on Windows) and quick links into `/usr/local/bin`
  (`C:\Program Files\R\bin` on Windows).
- **User mode**: rig installs everything into the user's home directory and
  never needs elevated privileges. R goes into `~/.local/share/rig/r`
  (`%APPDATA%\rig\data\r` on Windows) and quick links into `~/.local/bin`
  (`%USERPROFILE%\.local\bin` on Windows).

Mode resolution (`get_mode()` in `src/utils.rs`) checks, in order: the
`--user`/`--admin` global flags, the `RIG_MODE` environment variable, the
`mode` key in the rig config file, then defaults to admin. The mode is cached
after the first lookup.

Never hard-code mode-specific paths. Resolve directories through the helpers
in `src/utils.rs`, which are mode-aware and also honor override env vars /
config keys:

- `get_binary_dir()` — quick-link directory (`RIG_BINARY_DIR` / `binary-dir`).
- `get_r_install_dir()` — R installation root (`RIG_R_INSTALL_DIR` /
  `r-install-dir`).

`rig system user-mode` (`sc_system_user_mode` in `src/macos.rs`,
`src/linux.rs` and `src/windows/mod.rs`) switches an existing admin-mode setup
to user mode, reinstalls the R versions, and cleans up the admin-mode files.
The actual removal of the system-wide installations and links is delegated to
the hidden `rig system clean-admin-r` command, which self-escalates (`sudo` on
Unix, gsudo/UAC on Windows).

When editing docs or help text (`src/help-*.in`, the website under `website/`,
`README.md`), describe both modes; do not present admin-mode directories or the
`/usr/local/bin` binary location as the only behavior.

## Documentation website

The full user documentation is a Quarto website under `website/` (see
`website/_quarto.yml`). Prose lives in `website/_partials/*.md` (one markdown
file per section: `intro`, `features`, `known-issues`, `install`, `usage`,
`macos-app`, `docker`, `faq`, `feedback`); the `.qmd` pages are thin wrappers
that `{{< include >}}` a partial. Edit the partials, not the rendered HTML.

- The site is **one level deep**: `index.qmd` (Get started — intro, quick
  start, features, known issues) plus five flat Guide pages
  (`install.qmd`, `usage.qmd`, `macos-app.qmd`, `docker.qmd`, `faq.qmd`),
  `reference/index.qmd`, `articles/index.qmd` and `news.qmd`. Do **not** add a
  further level of sub-pages.
- The layout is the uv-style three-column docs layout: a **permanent docked
  left sidebar** holds all navigation (Get started, a collapsible `Guide`
  section with the five Guide pages, Reference, Articles, Changelog — see the
  `sidebar:` block in `_quarto.yml`), the content is in the middle, and the
  right-hand on-page TOC (`toc: true`) lists the current page's sections. The
  main navigation lives in the sidebar only; the top `navbar` is kept thin
  (search, GitHub link) so nav is not duplicated.
- The site has a light/dark theme toggle (`theme: { light: cosmo, dark:
  darkly }`). Shared cross-theme style overrides live in `website/theme.scss`
  (applied to both themes), e.g. pinning the navbar height.
- Cross-links between pages are `.qmd` links (e.g. `[FAQ](faq.qmd)`,
  `[list below](install.qmd#id-supported-linux-distributions)`).

- `README.md` is now an **ultra-minimal landing page** generated from
  `README.qmd` (which includes `website/_partials/intro.md` and
  `feedback.md`). It just describes rig and links to the website. Do **not**
  put full docs back in the README. Regenerate with `make readme`
  (`quarto render README.qmd --to gfm`).
- Build/preview the site with `make docs` / `make docs-preview` (or
  `quarto render website` / `quarto preview website`). No R or `cargo build`
  is needed — the content is static markdown.
- The site is deployed to the root of the GitHub Pages `gh-pages` branch on
  every push to the default branch, handled by `.github/workflows/docs.yml`.
- `website/news.qmd` includes the repo's `NEWS.md`; keep the changelog in
  `NEWS.md`. `website/reference/` and `website/articles/` hold the reference
  manual(s) and articles/blog-post listings.

## Build Commands

```bash
# Development build
cargo build

# Release build
cargo build --release

# Run tests
cargo test

# Format code
cargo fmt

# Lint code
cargo clippy
```

## Platform-Specific Builds (Makefile)

```bash
make macos                  # Build macOS packages (arm64 and x86_64)
make win                    # Build Windows installer
make linux                  # Build Linux packages (tar.gz, .deb, .rpm)
make linux-in-docker        # Build Linux packages in Docker
make clean                  # Clean build artifacts
```

## Testing

```bash
# Rust integration tests
cargo test

# Platform-specific shell tests (BATS)
bats tests/test-macos.sh
bats tests/test-linux.sh
bats tests/test-windows.sh

# Docker-based Linux tests
make linux-test-all
make linux-test-ubuntu-22.04  # or other distro names
```

## Architecture

### Platform Modules (Conditional Compilation)
The codebase uses `#[cfg(target_os = "...")]` for platform-specific code:
- `src/macos.rs` - macOS implementation
- `src/windows.rs` - Windows implementation
- `src/linux.rs` - Linux implementation

### Core Components
- `src/main.rs` - CLI entry point
- `src/lib.rs` - C API for macOS menu bar app (builds as `libriglib.a`)
- `src/args.rs` - Command-line argument parsing (clap)
- `src/common.rs` - Shared functionality across platforms

### Feature Modules
- `src/resolve.rs` - R version resolution (symbolic names like "release", "devel")
- `src/rversion.rs` - R version parsing
- `src/library.rs` - Package library management
- `src/repos.rs` - CRAN/PPM repository handling
- `src/download.rs` - HTTP downloads
- `src/alias.rs` - R version alias management
- `src/proj.rs` - Project dependency management
- `src/solver.rs` - Dependency solver (pubgrub algorithm)
- `src/dcf.rs` - DCF (DESCRIPTION) file parsing

### Utilities
- `src/escalate.rs` - Privilege escalation (sudo/admin)
- `src/config.rs` - Configuration management
- `src/run.rs` - Running R scripts
- `src/renv.rs` - renv integration

## Build Artifacts

The project produces two artifacts:
1. `rig` binary - the main CLI tool
2. `libriglib.a` static library - used by the macOS menu bar app

Shell completions (bash, zsh, fish, elvish, PowerShell) are auto-generated during build via `build.rs`.

## Key Dependencies

- **clap** - CLI argument parsing
- **reqwest** - HTTP client (with rustls-tls)
- **tokio** - Async runtime
- **pubgrub** - Dependency solver algorithm
- **deb822-fast** - DCF/DESCRIPTION file parsing
- **duct** - External command execution
