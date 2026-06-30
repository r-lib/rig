Use `rig add` to add a new R installation:

```
rig add release
```

Use `rig list` to list the currently installed R versions, and `rig default`
to set the default one.

Run `rig` to see all commands and examples.

Run `rig --help` and `rig <subcommand> --help` to see the documentation.

## Command list

```
rig add        -- install a new R version [alias: install]
rig available  -- list R versions available to install.
rig default    -- print or set default R version [alias: switch]
rig library    -- manage package libraries [alias: lib] (experimental)
rig list       -- list installed R versions [alias: ls]
rig repos      -- manage package repositories
rig resolve    -- resolve a symbolic R version
rig rm         -- remove R versions [aliases: del, remove, delete]
rig rstudio    -- start RStudio with specified R version
rig rtools     -- manage Rtools installations (on Windows only)
rig run        -- run R, an R script or an R project
rig sysreqs    -- manage R-related system libraries and tools (experimental)
rig system     -- manage current installations
```

Run `rig <subcommand> --help` for information about a subcommand.

## macOS `rig system` subcommands

```
rig system add-pak                 -- install or update pak for an R version
rig system allow-core-dumps        -- allow creating core dumps when R crashes
rig system allow-debugger          -- allow debugging R with lldb and gdb
rig system allow-debugger-rstudio  -- allow debugging RStudio with lldb and gdb
rig detect-platform                -- detect operating system version and distribution
rig system fix-permissions         -- restrict system library permissions to admin
rig system forget                  -- make system forget about R installations
rig system make-links              -- create R-* quick links
rig system make-orthogonal         -- make installed versions orthogonal
rig system no-openmp               -- remove OpenMP (-fopenmp) option for Apple compilers
rig system setup-user-lib          -- set up automatic user package libraries [alias: create-lib]
rig system user-mode               -- switch to user mode and clean up admin-mode installations
```

## Windows `rig system` subcommands

```
rig system add-pak                 -- install or update pak for an R version
rig system clean-registry          -- clean stale R related entries in the registry
rig detect-platform                -- detect operating system version and distribution
rig system make-links              -- create R-* quick links
rig system setup-user-lib          -- set up automatic user package libraries [alias: create-lib]
rig system update-rtools40         -- update Rtools40 MSYS2 packages
rig system user-mode               -- switch to user mode and clean up admin-mode installations
```

## Linux `rig system` subcommands

```
rig system add-pak                 -- install or update pak for an R version
rig system detect-platform         -- detect operating system version and distribution
rig system make-links              -- create R-* quick links
rig system setup-user-lib          -- set up automatic user package libraries [alias: create-lib]
rig system update-certs            -- download the CA certificate bundle and configure R to use it
rig system user-mode               -- switch to user mode and clean up admin-mode installations
```
