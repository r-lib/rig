## rig repositories start
## rig repositories version 2

invisible(local({
    # this is to undo the default set by earlier versions of rig
    options(repos = NULL)
    # only really needed for P3M, but it is a better default, so set it everywhere
    rver <- getRversion()
    ua <- sprintf(
        "R/%s R (%s)",
        rver,
        paste(
            rver,
            R.version$platform,
            R.version$arch,
            R.version$os
        )
    )
    options(HTTPUserAgent = ua)
    do <- function() {
        # Don't do anything at all if R_REPOSITORIES is set.
        # Clearly the user wants to manage repos themselves.
        if (Sys.getenv("R_REPOSITORIES") != "") {
            return()
        }
        # Don't do anything if not in RStudio/Positron and R >= 4.3.0.
        # In this case R (.onLoad() in utils) will load and set the repos.
        rstudio <- Sys.getenv("RSTUDIO") != ""
        positron <- Sys.getenv("POSITRON") != ""
        if (rver >= "4.3.0" && !rstudio && !positron) {
            return()
        }
        # If not RStudio/Positron, and R < 4.3.0, then set a load hook
        # on utils to read and set the repos.
        if (rver < "4.3.0" && !rstudio && !positron) {
            setHook(packageEvent("utils", "onLoad"), function(...) {
                reposdf <- tools:::.get_repositories()
                reposdf <- reposdf[reposdf$default, , drop = FALSE]
                repos <- structure(reposdf$URL, names = row.names(reposdf))
                # this should not happen, nevertheless...
                if (is.na(match("CRAN", names(repos)))) {
                    repos <- c(CRAN = "@CRAN@", repos)
                }
                options(repos = repos)
            })
        } else if ((rstudio || positron) && rver >= "4.3.0") {
            # If RStudio/Positron and R >= 4.3.0, then set option(repos)
            # to c(CRAN = "@CRAN@") to avoid utils reading the rig-updated
            # repositories file.
            # If that file is not by rig then we don't need this.
            repositories <- R.home("etc/repositories")
            lns <- readLines(repositories, warn = FALSE)
            if (any(grepl("added by rig", lns, fixed = TRUE))) {
                options(repos = c(CRAN = "@CRAN@"))
            }
        } else if ((rstudio || positron) && rver < "4.3.0") {
            # If RStudio/Positron and R < 4.3.0, then we don't need to do anything,
            # utils does not load the repositories in this case.
        }
    }
    tryCatch(do(), error = function(e) {
        warning("Could not load repositories file: ", e$message)
    })
}))

## rig repositories end
