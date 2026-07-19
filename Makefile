
VERSION=$(shell grep "^version" Cargo.toml | head -1 | tr -cd '0-9.')
SOURCES=$(wildcard src/*.rs) $(wildcard src/*.in)

all:
	@echo "Call 'make win', 'make macos' or 'make linux'"

Rig.app:
	cargo build --lib --release
	cargo build --lib --target x86_64-apple-darwin --release
	cbindgen -l c > Rig.App/Rig/rig.h
	mkdir -p Rig.app/lib
	lipo target/release/libriglib.a \
		target/x86_64-apple-darwin/release/libriglib.a \
		-create -output Rig.app/lib/libriglib.a
	cd Rig.app && xcodebuild -configuration Release -scheme Rig -derivedDataPath build-x86_64 -arch x86_64 clean build
	cd Rig.app && xcodebuild -configuration Release -scheme Rig -derivedDataPath build-arm64 -arch arm64 clean build

Rig.app/build-arm64/Build/Products/Release/Rig.app: Rig.app

# -------------------------------------------------------------------------

win: rig-$(VERSION).exe win-zip

rig-$(VERSION).exe: target/release/rig.exe rig.iss gsudo.exe
	find target/release -name _rig.ps1 -exec cp \{\} _rig.ps1 \;
	"C:\Program Files (x86)\Inno Setup 6\ISCC.exe" rig.iss
	cp output\mysetup.exe $@

# User-mode archive for Windows: rig.exe + gsudo.exe (so admin-mode
# escalation still works) + shell completions, laid out to be extracted
# into %USERPROFILE%\.local by an unprivileged user. No Inno Setup, no
# elevation. Uses 7-Zip, which is preinstalled on GitHub Windows runners.
win-zip: rig-$(VERSION)-windows.zip

rig-$(VERSION)-windows.zip: target/release/rig.exe gsudo.exe
	rm -rf zipdir
	mkdir -p zipdir/bin \
	    zipdir/share/bash-completion/completions \
	    zipdir/share/elvish/lib \
	    zipdir/share/fish/vendor_completions.d \
	    zipdir/share/zsh/site-functions \
	    zipdir/share/rig
	cp target/release/rig.exe zipdir/bin/
	cp gsudo.exe zipdir/bin/
	find target/release/build -name rig.bash -exec cp \{\} zipdir/share/bash-completion/completions \;
	find target/release/build -name rig.elv -exec cp \{\} zipdir/share/elvish/lib \;
	find target/release/build -name rig.fish -exec cp \{\} zipdir/share/fish/vendor_completions.d \;
	find target/release/build -name _rig -exec cp \{\} zipdir/share/zsh/site-functions \;
	find target/release/build -name _rig.ps1 -exec cp \{\} zipdir/share/rig \;
	rm -f $@
	cd zipdir && 7z a ../$@ bin share

gsudo.exe:
	mkdir -p gsudo
	curl -L https://github.com/gerardog/gsudo/releases/download/v2.6.1/gsudo.portable.zip -o gsudo/gsudo.zip
	cd gsudo && unzip -o gsudo.zip
	cp gsudo/x64/gsudo.exe .

# -------------------------------------------------------------------------

ifeq "$(DOCKER_DEFAULT_PLATFORM)" ""
    DOCKER_ARCH :=
else
    DOCKER_ARCH := --platform=$(DOCKER_DEFAULT_PLATFORM)
endif

linux: export OPENSSL_DIR = /usr/local/
linux: export OPENSSL_INCLUDE_DIR = /usr/local/include/
linux: export OPENSSL_LIB_DIR = /usr/local/lib/
linux: export OPENSSL_STATIC = 1
linux: export DEP_OPENSSL_INCLUDE = /usr/local/include/
linux: rig-$(VERSION).tar.gz r-rig-$(VERSION).deb r-rig-$(VERSION).rpm

rig-$(VERSION).tar.gz: target/release/rig
	ls -l target/release/rig
	strip -x target/release/rig
	mkdir -p build/bin
	mkdir -p build/share/bash-completion/completions
	mkdir -p build/share/elvish/lib
	mkdir -p build/share/fish/vendor_completions.d
	mkdir -p build/share/zsh/site-functions
	ls -l target/release
	cp target/release/rig build/bin
	find target/release/build -name rig.bash -exec cp \{\} build/share/bash-completion/completions \;
	find target/release/build -name rig.elv -exec cp \{\} build/share/elvish/lib \;
	find target/release/build -name rig.fish -exec cp \{\} build/share/fish/vendor_completions.d \;
	find target/release/build -name _rig -exec cp \{\} build/share/zsh/site-functions \;
	mkdir -p build/share/rig
	curl -L -o build/share/rig/cacert.pem 'https://curl.se/ca/cacert.pem'
	tar cz -C build -f $@ bin share

r-rig-$(VERSION).deb: rig-$(VERSION).tar.gz tools/linux/make-deb.sh
	VERSION=$(VERSION) ./tools/linux/make-deb.sh $< $@

r-rig-$(VERSION).rpm: rig-$(VERSION).tar.gz tools/linux/make-rpm.sh
	VERSION=$(VERSION) ./tools/linux/make-rpm.sh $< $@

shell-alpine:
	docker compose build alpine
	docker run -ti -v .:/work \
		-e LOCAL_UID=`id -u` -e LOCAL_GID=`id -g` $(DOCKER_ARCH) \
		-v rig-linux-target-alpine:/work/target \
		-v cargo-cache:/cargo \
		rlib/rig-alpine:latest bash

shell-ubuntu:
	docker compose build ubuntu
	docker run -ti -v .:/work \
		-e LOCAL_UID=`id -u` -e LOCAL_GID=`id -g` $(DOCKER_ARCH) \
		-v rig-linux-target-ubuntu:/work/target \
		-v cargo-cache:/cargo \
		rlib/rig-ubuntu:latest bash

linux-amd64-in-docker:
	@echo "make linux-amd64-in-docker is only reliable after make clean"
	DOCKER_DEFAULT_PLATFORM=linux/amd64 make linux-in-docker

linux-arm64-in-docker:
	@echo "make linux-arm64-in-docker is only reliable after make clean"
	DOCKER_DEFAULT_PLATFORM=linux/arm64 make linux-in-docker

linux-in-docker:
	docker compose build
	docker run -v .:/work \
		-e LOCAL_UID=`id -u` -e LOCAL_GID=`id -g` \
		rlib/rig-alpine:latest make linux

VARIANTS = ubuntu-20.04 ubuntu-22.04 ubuntu-24.04 ubuntu-26.04 debian-12 debian-13 rockylinux/rockylinux-8 rockylinux/rockylinux-9 rockylinux/rockylinux-10 opensuse/leap-15.6 fedora-42 fedora-43 almalinux-8 almalinux-9 almalinux-10 redhat/ubi8 redhat/ubi9 redhat/ubi10
print-linux-variants:
	@echo $(VARIANTS)
print-linux-variants-json:
	@echo $(VARIANTS) | sed 's/ /","/g' | sed 's/^/["/' | sed 's/$$/"]/'

ENVS = -e REDHAT_ORG_RHEL7=$(REDHAT_ORG) \
       -e REDHAT_ORG_RHEL8=$(REDHAT_ORG) \
       -e REDHAT_ORG_RHEL9=$(REDHAT_ORG) \
       -e REDHAT_ORG_RHEL10=$(REDHAT_ORG) \
       -e REDHAT_ACTIVATION_KEY_RHEL7=$(REDHAT_ACTIVATION_KEY_RHEL7) \
       -e REDHAT_ACTIVATION_KEY_RHEL8=$(REDHAT_ACTIVATION_KEY_RHEL8) \
       -e REDHAT_ACTIVATION_KEY_RHEL9=$(REDHAT_ACTIVATION_KEY_RHEL9) \
       -e REDHAT_ACTIVATION_KEY_RHEL10=$(REDHAT_ACTIVATION_KEY_RHEL10)

define GEN_TESTS
linux-test-$(variant):
	mkdir -p tests/results
	rm -f tests/results/`echo $(variant) | tr / -`.fail \
	      tests/results/`echo $(variant) | tr / -`.success
	docker run -t --rm $(DOCKER_ARCH) --privileged \
		-v $(PWD):/work $(ENVS) `echo $(variant) | tr - :` \
		bash -c /work/tests/test-linux-docker.sh && \
	touch tests/results/`echo $(variant) | tr / -`.success || \
	touch tests/results/`echo $(variant) | tr / -`.fail
shell-$(variant):
	docker run -ti --rm -v $(PWD):/work $(ENVS) `echo $(variant) | tr - :` bash
.PHONY: linux-test-$(variant) shell-$(variant)
TEST_IMAGES += linux-test-$(variant)
endef
$(foreach variant, $(VARIANTS), $(eval $(GEN_TESTS)))

linux-test-all: $(TEST_IMAGES)
	if ls tests/results | grep -q fail; then \
		echo Some tests failed; \
		ls tests/results; \
		exit 1; \
	fi

# -------------------------------------------------------------------------

macos: release

target/release/rig.exe: $(SOURCES)
	rm -rf target/release/build/rig-*
	cargo build --release

target/release/rig: $(SOURCES)
	rm -rf target/release/build/rig-*
	cargo build --release

target/x86_64-apple-darwin/release/rig: $(SOURCES)
	rm -rf target/x86_64-apple-darwin/release/build/rig-*
	cargo build --target x86_64-apple-darwin --release

release: rig-$(VERSION)-macOS-arm64.pkg rig-$(VERSION)-macOS-x86_64.pkg \
	 rig-$(VERSION)-macOS-arm64.tar.gz rig-$(VERSION)-macOS-x86_64.tar.gz

rig-$(VERSION)-macOS-%.pkg: rig-unnotarized-%.pkg tools/gon.hcl.in
	if [[ "x$$AC_PASSWORD" == "x" ]]; then \
		echo "AC_PASSWORD is not set"; \
		exit 2; \
	fi
	cat tools/gon.hcl.in | \
		sed 's/{{VERSION}}/$(VERSION)/g' | \
		sed 's/{{ARCH}}/$*/g' | \
		sed 's/{{AC_PASSWORD}}/'$$AC_PASSWORD'/g' | \
		sed 's/{{TEAM_ID}}/'$$TEAM_ID'/g' > tools/gon.hcl
	cp $< $@
	gon -log-level=warn ./tools/gon.hcl

rig-unnotarized-%.pkg: build.stamp tools/distribution.xml.in
	codesign --force \
		--options runtime \
		-s 'Developer ID Application: Gabor Csardi (5CK56ZU9YH)' \
		build-$*/Applications/Rig.app
	codesign --force \
		--options runtime \
		-s 'Developer ID Application: Gabor Csardi (5CK56ZU9YH)' \
		build-$*/usr/local/bin/rig
	pkgbuild --root build-$* \
		--identifier com.gaborcsardi.rig \
		--version $(VERSION) \
		--ownership recommended \
		rig-$*.pkg
	cat tools/distribution.xml.in | sed "s/{{VERSION}}/$(VERSION)/g" | \
		 sed "s/{{ARCH}}/$*/g" > tools/distribution.xml
	productbuild --distribution tools/distribution.xml \
		--resources Resources \
		--package-path rig-$*.pkg \
		--version $(VERSION) \
		--sign "Developer ID Installer: Gabor Csardi" $@

macos-unsigned: rig-$(VERSION)-macOS-unsigned-arm64.pkg rig-$(VERSION)-macOS-unsigned-x86_64.pkg

macos-unsigned-x86_64: rig-$(VERSION)-macOS-unsigned-x86_64.pkg

macos-unsigned-arm64: rig-$(VERSION)-macOS-unsigned-arm64.pkg

rig-$(VERSION)-macOS-unsigned-%.pkg: build.stamp tools/distribution.xml.in
	pkgbuild --root build-$* \
		--identifier com.gaborcsardi.rig \
		--version $(VERSION) \
		--ownership recommended \
		$@
	cat tools/distribution.xml.in | sed "s/{{VERSION}}/$(VERSION)/g" | \
		 sed "s/{{ARCH}}/$*/g" > tools/distribution.xml

# ------------------------------------------------------------------------
# User-mode archives for macOS.
#
# These mirror the Linux tarball layout (bin/rig + share/** completions,
# no Linux-only cacert.pem) and are meant to be extracted into ~/.local by
# an unprivileged user. `macos_tarball` assembles one archive; $(1) is the
# source `rig` binary and $(2) the output tar.gz.
define macos_tarball
	rm -rf tarball-tmp
	mkdir -p tarball-tmp/bin \
	    tarball-tmp/share/bash-completion/completions \
	    tarball-tmp/share/elvish/lib \
	    tarball-tmp/share/fish/vendor_completions.d \
	    tarball-tmp/share/zsh/site-functions
	cp $(1) tarball-tmp/bin/rig
	find target/release/build -name rig.bash -exec cp \{\} tarball-tmp/share/bash-completion/completions \;
	find target/release/build -name rig.elv -exec cp \{\} tarball-tmp/share/elvish/lib \;
	find target/release/build -name rig.fish -exec cp \{\} tarball-tmp/share/fish/vendor_completions.d \;
	find target/release/build -name _rig -exec cp \{\} tarball-tmp/share/zsh/site-functions \;
	tar cz -C tarball-tmp -f $(2) bin share
	rm -rf tarball-tmp
endef

# Credential-free host-architecture tarball, built straight from
# `cargo build --release`. This is what CI produces and tests; no pkg,
# no signing, no notarization, no xcodebuild.
macos-tarball: rig-$(VERSION)-macos.tar.gz

rig-$(VERSION)-macos.tar.gz: target/release/rig
	strip -x target/release/rig
	$(call macos_tarball,target/release/rig,$@)

# Signed + notarized per-arch release tarballs. These depend on the
# notarized .pkg so the binary they package is the one whose cdhash was
# registered with Apple (it passes Gatekeeper's online check even as a
# loose binary). Requires signing credentials; run as part of `make macos`.
rig-$(VERSION)-macOS-%.tar.gz: rig-$(VERSION)-macOS-%.pkg
	$(call macos_tarball,build-$*/usr/local/bin/rig,$@)

# README.md is the short landing page, generated from README.qmd (which
# includes shared partials from website/_partials). The full documentation
# lives in the Quarto website under website/.
README.md: README.qmd website/_partials/intro.md website/_partials/feedback.md
	quarto render README.qmd --to gfm

.PHONY: help readme docs docs-preview cli-reference
# Regenerate src/help-generated.in (the colored ANSI short `about` and long
# `--help` strings) from the Markdown sources in src/help/*.md. Each file's lead
# paragraph is the short summary, the rest is the long help. Run after editing
# any of those files.
help:
	cargo xtask gen-help

readme: README.md

# Regenerate the command-line reference (website/reference/_commands.md) from
# the built rig binary. Requires a release (or debug) build of rig.
cli-reference:
	website/gen-cli-reference.sh

# Build the documentation website into website/_site.
docs: cli-reference
	quarto render website

# Live-preview the documentation website.
docs-preview:
	quarto preview website

build.stamp: target/release/rig target/x86_64-apple-darwin/release/rig \
	     Rig.app/build-arm64/Build/Products/Release/Rig.app \
	     Rig.app/build-x86_64/Build/Products/Release/Rig.app
	rm -rf build-arm64 build-x86_64
	# arm64
	mkdir -p build-arm64/usr/local/bin
	mkdir -p build-arm64/usr/local/share/zsh/site-functions
	mkdir -p build-arm64/opt/homebrew/etc/bash_completion.d/
	mkdir -p build-arm64/opt/homebrew/share/elvish/lib
	mkdir -p build-arm64/opt/homebrew/share/fish/vendor_completions.d/
	cp target/release/rig build-arm64/usr/local/bin/
	strip -x build-arm64/usr/local/bin/rig
	find target/release/build -name _rig -exec cp \{\} build-arm64/usr/local/share/zsh/site-functions \;
	find target/release/build -name rig.bash -exec cp \{\} build-arm64/opt/homebrew/etc/bash_completion.d \;
	find target/release/build -name rig.elv -exec cp \{\} build-arm64/opt/homebrew/share/elvish/lib \;
	find target/release/build -name rig.fish -exec cp \{\} build-arm64/opt/homebrew/share/fish/vendor_completions.d \;
	# x86_64
	mkdir -p build-x86_64/usr/local/bin
	mkdir -p build-x86_64/usr/local/share/zsh/site-functions
	mkdir -p build-x86_64/opt/homebrew/etc/bash_completion.d/
	mkdir -p build-x86_64/opt/homebrew/share/elvish/lib
	mkdir -p build-x86_64/opt/homebrew/share/fish/vendor_completions.d/
	cp target/x86_64-apple-darwin/release/rig build-x86_64/usr/local/bin/
	strip -x build-x86_64/usr/local/bin/rig
	find target/release/build -name _rig -exec cp \{\} build-x86_64/usr/local/share/zsh/site-functions \;
	find target/release/build -name rig.bash -exec cp \{\} build-x86_64/opt/homebrew/etc/bash_completion.d \;
	find target/release/build -name rig.elv -exec cp \{\} build-x86_64/opt/homebrew/share/elvish/lib \;
	find target/release/build -name rig.fish -exec cp \{\} build-x86_64/opt/homebrew/share/fish/vendor_completions.d \;
	# Rig.app
	mkdir build-arm64/Applications
	mkdir build-x86_64/Applications
	cp -r Rig.app/build-arm64/Build/Products/Release/Rig.app build-arm64/Applications/
	rm -rf build-arm64/Applications/Rig.app/Contents/Resources/LaunchAtLogin_LaunchAtLogin.bundle
	cp -r Rig.app/build-x86_64/Build/Products/Release/Rig.app build-x86_64/Applications/
	rm -rf build-x86_64/Applications/Rig.app/Contents/Resources/LaunchAtLogin_LaunchAtLogin.bundle
	# Resources
	rm -rf Resources
	mkdir Resources
	cp README.md NEWS.md LICENSE Resources/
	touch $@

# -------------------------------------------------------------------------

coverage:
	rustup component add llvm-tools-preview
	cargo llvm-cov --html --open

coverage-ci:
	rustup component add llvm-tools-preview
	cargo llvm-cov --lcov --output-path lcov.info

.PHONY: release clean all macos macos-tarball win win-zip linux Rig.app \
	shell-linux linux-in-docker linux-amd64-in-docker linux-arm64-in-docker \
	linux-test-all coverage coverage-ci

clean:
	cargo clean
	rm -rf build.stamp build-* Resources *.pkg tools/distribution.xml \
		tools/gon.hcl Output *.exe *.deb *.rpm *.tar.gz *.zip build \
		tarball-tmp zipdir
