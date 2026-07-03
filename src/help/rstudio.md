## Description

Start RStudio with the specified R version.

If `project-file` is an RStudio `.Rproj` file, or a directory containing
an RStudio project file, RStudio will open the project.

If `project-file` is a directory that does not contain an RStudio
project file, then RStudio will start up without an active project,
but will set the working directory to the specified directory.

If `project-file` is a regular file that is not inside a directory
containing an RStudio project, then RStudio will start up without an
active project, but it will open the specified file, and will set the
working directory to the directory of the file.

If the RStudio project or the specified directory contains an `renv.lock`
file (created by the renv package), and `version` is not specified, then
rig will read the preferred R version from the `renv.lock` file.
If the same exact version is not installed, then rig chooses
the latest version with the same major and minor components. If no such
version is available, rig throws an error.

On macOS arm64 computers rig prefers arm64 R, unless an exact version
match is only available with x86_64 R.

On Windows, `rig rstudio` needs RStudio Desktop 2021.09.0+351 or later.

## Examples

```sh
# With default R version
rig rstudio

# With another R version
rig rstudio 4.6

# Open project with default R version
rig rstudio cli.Rproj

# Open renv project with the R version specified in the lock file
rig rstudio projects/myproject/renv.lock

# Open RStudio project with specified R version, either is good
rig rstudio 4.6 cli.Rproj
rig rstudio cli.Rproj 4.0
```
