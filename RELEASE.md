
- [ ] Make sure README has the correct command list.
- [ ] Make sure version number is updated everywhere:
  - Cargo.toml
  - Cargo.lock (run cargo build)
  - src/args.rs
  - rig.iss
  - NEWS file
  - README.Rmd
- [ ] Make sure CI is OK
- [ ] If needed, commit to to have a CI build with the right version number.
- [ ] Build README:
  ```
  Rscript -e 'rmarkdown::render("README.Rmd")'
  ```
- [ ] Update NEWS header to remove `(not released yet)`
- [ ] Build signed and notarized macOS packages locally:
  ```
  export AC_PASSWORD=...
  sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
  make clean
  make macos
  ```
- [ ] Download the artifacts for the new version for Windows & Linux (x2)
- [ ] Create tag for the current version, push to GH.
- [ ] Create release on GH, add the installers.
- [ ] Test installers.
- [ ] `git commit` with the NEWS and README updates, update tag, push to GH,
      `--tags` as well.
- [ ] Update homebrew repo.
- [ ] Update choco package.
