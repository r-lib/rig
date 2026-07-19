Create R-* quick links

## Description

Create quick links in the rig binary directory (`~/.local/bin` in [user
mode](../articles/admin-vs-user-mode.qmd)) for the current R installations. These let you directly run a
specific R version. E.g. `R-4.6.0` will start R 4.6.0. It also creates
the `R` and `Rscript` links if they are missing.

`rig add` runs `rig system make-links`, so if you only use rig to
install R, then you do not need to run it manually.

In user mode no administrator rights are needed. In admin mode you need
an administrator account to run this command or use `sudo` on Unix,
otherwise rig will ask for your password.
