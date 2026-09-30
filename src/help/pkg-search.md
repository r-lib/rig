Search for CRAN packages

## Description

Search CRAN packages by name, title, description, author and maintainer. The
search runs on the web service of the pkgsearch R package,
<https://search.r-pkg.org>, so it gives the same results as
`pkgsearch::pkg_search()`, and it needs network access. It always searches
CRAN, not the repositories configured with [`rig repos`](repos.qmd). This
command does not start R.

Packages that match all search terms, or the whole phrase, rank higher, and
packages with more reverse dependencies rank higher, too.

Example output:
```
- "permutation test" ------------------------- 3431 packages in 0.023 seconds -

1 coin @ 1.4-5 (score 100)                        Torsten Hothorn, 3 months ago
--------------
  # Conditional Inference Procedures in a Permutation Test Framework
  Conditional inference procedures for the general independence problem
  including two-sample, K-sample (non-parametric ANOVA), correlation,
  censored, ordered and multivariate problems described in
  <doi:10.18637/jss.v028.i08>.
  https://codeberg.org/thothorn/coin/

2 exactRankTests @ 0.8-37 (score 33)              Torsten Hothorn, 5 months ago
-------------------------
  # Exact Distributions for Rank and Permutation Tests
  Computes exact conditional p-values and quantiles using an
  implementation of the Shift-Algorithm by Streitberg & Roehmel.

...

Next 8 results: rig pkg search "permutation test" --from 9
```

With `--short` (`-s`) every package takes a single line:
```
- "permutation test" ------------------------- 3431 packages in 0.017 seconds -
 #     package           version by                      @ title
 1 100 coin              1.4-5   Torsten Hothorn        3M Conditional Infer...
 2  33 exactRankTests    0.8-37  Torsten Hothorn        5M Exact Distributio...
 3  33 lmPerm            2.1.6   Marco Torchiano       10M Permutation Tests...
...

Next 20 results: rig pkg search "permutation test" --from 21 --short
```

The score of a package is a percentage of the score of the best match. The
long format shows it after the version, the short format in the unnamed
second column. The `@` column is the time since the package was
last released.

Multiple words are searched together, so `rig pkg search permutation test`
is the same as `rig pkg search "permutation test"`.

`--size` (`-n`) sets the number of results to show, the default is 8, or
20 with `--short`. `--from` shows results starting from this rank, e.g.
`--from 9` shows the second page of results in the long format. If there
are more results, the last line of the output shows the command that prints
the next page.

`--json` prints the results in JSON, including more fields: the maintainer's
email address, the number of reverse dependencies, the number of downloads in
the last month, the license, the URLs and the bug report URL.

In a terminal, the output is shown through a pager: `$RIG_PAGER`, `$PAGER`
or `less`, in this order. Set `RIG_PAGER` to `cat` to turn this off.
