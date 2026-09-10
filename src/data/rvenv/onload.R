# Rest of the shim package's `.onLoad()` (see `.rvenvlib/rvenv/R/rvenv`),
# sourced by it with `local = TRUE` once `.rvenv/lib` is known to exist. This
# file is rewritten by every `rig proj sync`, so it reuses the caller's
# `root`/`venv`/`lib`/`libname`/`activated` locals rather than recomputing
# them, and calls `restore_r_libs_user()` / `warn_unsynced()`, defined there.

# Reads `.rvenv/rvenv.cfg`, written by `rig proj sync`, as a named list of
# its `key = value` lines. `NULL` if the project has never been synced by a
# rig new enough to write the file.
read_rvenv_cfg <- function(venv) {
  cfg <- file.path(venv, "rvenv.cfg")
  if (!file.exists(cfg)) return(NULL)
  lines <- readLines(cfg, warn = FALSE)
  lines <- lines[!grepl("^\\s*#", lines) & nzchar(trimws(lines))]
  kv <- strsplit(lines, "\\s*=\\s*")
  keys <- vapply(kv, `[`, character(1), 1L)
  vals <- vapply(kv, `[`, character(1), 2L)
  as.list(stats::setNames(vals, keys))
}

# The running R's `<major>.<minor>`, e.g. "4.4" for R 4.4.1, matching the
# `r-minor` key `rig proj sync` writes to `rvenv.cfg` -- packages are tied to
# the R minor version, not the patch version.
r_minor_version <- function() {
  minor <- strsplit(R.version$minor, ".", fixed = TRUE)[[1]][1]
  paste(R.version$major, minor, sep = ".")
}

# Warns that the project library was built for a different R minor version
# than the one that is running, so it was not attached.
warn_wrong_r_version <- function(want, running) {
  packageStartupMessage(sprintf(
    "! Project needs R %s, this is R %s.",
    want,
    running
  ))
  packageStartupMessage(
    "  Switch R version, or run `rig proj sync` to rebuild for this R."
  )
  invisible()
}

# A project library built for a different R minor version is binary
# incompatible: its compiled packages may fail to load, or worse, load and
# crash. So treat a mismatch the same way as a missing library -- back off
# instead of attaching it -- rather than merely warning and continuing.
cfg <- read_rvenv_cfg(venv)
running <- r_minor_version()
if (!is.null(cfg[["r-minor"]]) && !identical(cfg[["r-minor"]], running)) {
  restore_r_libs_user(libname, lib)
  Sys.unsetenv("R_DEFAULT_PACKAGES")
  warned <- identical(
    normalizePath(Sys.getenv("RVENV_UNSYNCED"), mustWork = FALSE),
    venv
  )
  Sys.setenv(RVENV_UNSYNCED = venv)
  if (!activated && !warned) {
    warn_wrong_r_version(cfg[["r-minor"]], running)
  }
  return(invisible())
}

# A project that was synced since the parent process started.
Sys.unsetenv("RVENV_UNSYNCED")

Sys.setenv(
  R_LIBS_USER = lib,
  # Setting this empty does not reliably disable the site library on all R
  # versions, so point it at a path that does not exist.
  R_LIBS_SITE = "/nonexistent/rvenv-no-site",
  RVENV = venv
)

# Shared dev-tool library (devtools, usethis, roxygen2, ...), same for
# every project on this R version. `rig proj sync` resolves it once, into
# `rvenv.cfg`'s `tools-lib` -- not derived here from `RVENV_R_LIBS_USER`,
# because `rig run`'s wrapper sets `R_LIBS_USER` to the project library
# before R starts, and by the time R gets to snapshot its own default user
# library into `RVENV_R_LIBS_USER`, it is already gone. Appended after the
# project library, so project deps always resolve first; before `.Library`,
# which `.libPaths()` adds itself. Fine if it does not exist yet, or if an
# older `rig proj sync` never wrote it (empty string): `.libPaths()`
# silently drops missing entries.
paths <- lib
tools <- cfg[["tools-lib"]]
if (!is.null(tools) && nzchar(tools)) paths <- c(paths, tools)

# `include.site` was added in R 4.2.0.
if (getRversion() >= "4.2.0") {
  .libPaths(paths, include.site = FALSE)
} else {
  .libPaths(paths)
}

Sys.unsetenv("R_DEFAULT_PACKAGES")

# One warning per project is enough; child processes are quiet.
if (!activated) warn_unsynced(venv, lib)
