# This package is loaded from a project's .Renviron, via
#
#     R_DEFAULT_PACKAGES=rig,datasets,utils,grDevices,graphics,stats,methods
#
# It is the "in-session activation" leg of a rig project: it makes the
# project's `.rvenv/lib` library work in R sessions that rig did not start,
# e.g. in RStudio, Positron or VS Code. We deliberately do not use a project
# `.Rprofile` for this, because that would shadow the user's own
# `~/.Rprofile` entirely.
#
# `.Renviron` sets `R_LIBS_USER` to the *relative* path `.rvenv/lib`, because
# the file is committed to version control and has to work from any clone
# location. `.onLoad()` receives an already-resolved absolute `libname`, so
# this is where the relative path becomes an absolute one. That matters for
# child processes (callr, parallel, `R CMD`, `Rscript` from a subdirectory):
# they inherit the environment variable, not our `.libPaths()` call, so a
# child started in a subdirectory would otherwise look for
# `<subdir>/.rvenv/lib`.

.onLoad <- function(libname, pkgname) {
  venv <- normalizePath(dirname(libname), mustWork = FALSE)
  lib <- file.path(venv, "lib")

  # Whether a parent process activated this project already. Note that this
  # does *not* mean there is nothing to do: a child process started in the
  # project root reads the project .Renviron itself, which sets R_LIBS_USER
  # back to the relative `.rvenv/lib`, and a grandchild started from a
  # subdirectory would then look for the wrong directory. So the variables
  # below are re-asserted unconditionally, and only the parts that are not
  # idempotent -- re-reading the user's .Renviron and warning about an
  # unsynced project -- are skipped.
  activated <- identical(
    normalizePath(Sys.getenv("RVENV"), mustWork = FALSE),
    venv
  )

  # Re-read the user's own .Renviron first: the project .Renviron shadows it
  # rather than merging with it, so without this the user's variables are
  # empty in this session. Our own variables are set after it, so they win.
  # Not in a child process: there the parent's environment, including
  # whatever it deliberately changed, is what should survive.
  home_renv <- path.expand("~/.Renviron")
  if (!activated && file.exists(home_renv)) readRenviron(home_renv)

  Sys.setenv(
    R_LIBS_USER = lib,
    # Setting this empty does not reliably disable the site library on all R
    # versions, so point it at a path that does not exist.
    R_LIBS_SITE = "/nonexistent/rvenv-no-site",
    RVENV = venv
  )

  # `include.site` was added in R 4.2.0.
  if (getRversion() >= "4.2.0") {
    .libPaths(lib, include.site = FALSE)
  } else {
    .libPaths(lib)
  }

  Sys.unsetenv("R_DEFAULT_PACKAGES")

  # One warning per project is enough; child processes are quiet.
  if (activated) return(invisible())

  # `rig proj sync` copies the lock file it installed from to
  # `.rvenv/lib/.synced`. A copy rather than a hash, so that both sides only
  # need to read files: base R has no sha256, and md5 would mean one more
  # dependency on the rig side.
  stamp <- file.path(lib, ".synced")
  lock <- file.path(dirname(venv), "rproj.lock")
  synced <- FALSE
  if (file.exists(stamp) && file.exists(lock)) {
    synced <- identical(
      readLines(stamp, warn = FALSE),
      readLines(lock, warn = FALSE)
    )
  }
  if (!synced) {
    packageStartupMessage("! Project is not synced. Run: rig proj sync")
  }

  invisible()
}
