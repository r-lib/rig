Run `.R` files with rig from the Windows command line

## Description

Run `.R` files with rig from the Windows command line, e.g. `hello.R a b`
instead of `rig run hello.R a b`, in `cmd` and PowerShell.

This command associates `.R` files with rig, for the current user, and adds
`.R` to the user's `PATHEXT`, so Windows also finds `hello.R` on the `PATH`
as `hello`. It runs `rig run -f <file>` for a `.R` file.

This also changes what happens when you double-click a `.R` file: it runs
the script, instead of opening it in RStudio, Positron, RGui or another
editor.

If you chose an app for `.R` files in the *Open with* dialog of Windows,
then Windows uses that app, and rig cannot change it. rig warns about
this. To use rig, choose *R script (runs with rig)* in *Open with* >
*Choose another app*, with *Always*.

Open a new terminal for the `PATHEXT` change to take effect.

Use the `--undo` flag to remove the association, restore the previous
default app of `.R` files, and remove `.R` from `PATHEXT`.

This command does nothing on macOS and Linux.
