Delete cached files

## Description

Deletes files from rig's cache directory. With no options, deletes the
whole cache. Use `--category` to delete only one category of cached files
(`built`, `packages`, `metadata`, `p3m`, `git-mirrors`, `url-pkgs` or `scripts`), leaving
the rest in place. See [`rig cache info`](cache.qmd#rig-cache-info) for what each category contains.

If you keep the cache in a directory of your choice (see the `cache-dir`
[configuration entry](config.qmd)), rig will only clean it if it appears to be an actual
rig cache directory.
