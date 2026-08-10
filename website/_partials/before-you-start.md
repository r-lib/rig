## 0. Before you start

[Install rig](install.qmd) first. Check that rig works and see where it
puts things:

```sh
rig --version
rig system dirs
```

```
Mode          admin
Architecture  arm64
R root        /Library/Frameworks/R.framework/Versions
Binary dir    /usr/local/bin
...
```

[`rig system dirs`](reference/system.qmd#rig-system-dirs) works before any
R version is installed, so it is the quickest way to confirm which mode you
are in and where R will land. The directories depend on your platform, so
your output will differ.

Now choose your platform to continue.
