Information about the package in the repositories

## Description

Show information about a package on CRAN, from its `DESCRIPTION` file.

By default the latest available version is shown; use `--version` to
select a specific one, including versions that CRAN has archived. Use
`--json` to print all `DESCRIPTION` fields.

If CRAN has archived the package, i.e. removed it from the current
repository, rig shows the date it was archived, next to the publication
date of the version. `--json` reports it as an extra `Archived` field.

The README of the package is shown as well, when the repository has one.
Since this can be long, the output is paged through `$RIG_PAGER`,
`$PAGER`, or `less`, unless it is redirected to a file or a pipe. Set
`RIG_PAGER=cat` to turn paging off.

Markdown READMEs are rendered for the terminal. GitHub emoji shortcodes
like `:rocket:` become emoji, and links and images become clickable
terminal hyperlinks on their link text, so a badge shows as its name
instead of two long URLs. When the output is redirected, or `NO_COLOR` is
set, the link target is written out as `text (url)` instead. HTML
comments and tags are dropped, an `<img>` leaving its `alt` text behind,
and paragraphs are rewrapped to the width of the output.
