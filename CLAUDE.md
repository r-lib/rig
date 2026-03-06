# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**rig** is "The R Installation Manager" - a cross-platform CLI tool written in Rust that manages multiple R installations on macOS, Windows, and Linux. It allows users to install, remove, configure, and switch between different R versions.

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
