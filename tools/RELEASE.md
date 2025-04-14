
- [ ] Make sure README has the correct command list.
- [ ] Make sure version number is updated everywhere:
  - Cargo.toml
  - Cargo.lock (run cargo build)
  - rig.iss
  - choco/rig/rig.nuspec
  - NEWS file
- [ ] If needed, commit to have a CI build with the right version number.
- [ ] Trigger arm64 Linux CI build
- [ ] Make sure CI is OK
- [ ] Build README:
  ```
  Rscript -e 'rmarkdown::render("README.Rmd")'
  ```
- [ ] Update NEWS header to remove `(not released yet)`
- [ ] Build signed and notarized macOS packages locally:
  ```
  export AC_PASSWORD=...
  export TEAM_ID=...
  sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
  rm -rf target
  make clean
  make macos
  ```
  (https://github.com/mitchellh/gon is now archived, I had to compile my
  own fork, from https://github.com/UniversalMediaServer.)
- [ ] Download the artifacts for the new version for Windows & Linux (x2)
- [ ] Create tag for the current version, push to GH.
- [ ] Create release on GH, add the installers.
- [ ] Test the macOS installers. (The rest are tested in the CI.)
- [ ] `git commit` with the NEWS and README updates, update tag, push to GH,
      `--tags` as well.
- [ ] Update Debian repo, by running the Action manually, and then `pull --rebase` and push the
      `gh-pages` branch to the server at DigitalOcean.
- [ ] Update homebrew repo.
- [ ] Update choco package.
    - Make sure `rig.nuspec` is current
	- `choco pack`
	- Delete old `.nupkg` file
	- Test:
	  ```
	  gsudo choco uninstall rig
	  gsudo choco install rig --source .
	  rig --version
	  rig ls
	  rig available
	  ```
	- Submit:
	  ```
	  choco push rig.*.nupkg --source https://push.chocolatey.org/
	  ```
- [ ] Submit update to winget-pkgs:
    ```
	VERSION=x.y.z
    komac update --identifier  'Posit.rig' --version "$VERSION" \
    --urls "https://github.com/r-lib/rig/releases/download/v${VERSION}/rig-windows-${VERSION}.exe" \
    --submit
    ```
- [ ] Update the `latest` tag and release on GH.
- [ ] toot
