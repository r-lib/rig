Show download counts of packages

## Description

Show how many times packages were downloaded from the Package Manager
instance in the last 30 days, or in the last `--days` days.

`--daily` shows the downloads per day instead. This needs one request per day
and package.

Only instances that collect usage statistics have download counts. The
public Package Manager does not.

## Examples

```sh
rig ppm pkg downloads dplyr ggplot2
rig ppm pkg downloads dplyr --days 7 --daily
```
