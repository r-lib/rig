## Description

Run R, an R script or an R project, using the selected R version.

All of these examples allow an `--r-version` argument, to use a specific
R version.

```sh
rig run                    # start R
rig run -f <script-file>   # run an R script
rig run -e <expression>    # evaluate an R expression
rig run <pkg>::<script>    # run a script from a package's exec directory
rig run <path-to-app>      # run an R app
```

Currently supported apps are:

- Plumber APIs,
- Shiny apps,
- Quarto documents embedding Shiny apps,
- Quarto documents,
- Rmd documents,
- Rmd documents embedding Shiny apps,
- Static web sites.
