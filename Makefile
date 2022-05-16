
VERSION=$(shell grep "^version" Cargo.toml | tr -cd '0-9.')
SOURCES=$(wildcard src/*.rs) $(wildcard src/*.in)

all:
	@echo "Call 'make win', 'make macos' or 'make linux'"

rigx:
	cargo build --lib --release
	cargo build --lib --target x86_64-apple-darwin --release
	cbindgen -l c > include/rig.h > rigx/rigx/rig.h
	mkdir -p rigx/lib
	lipo target/release/libriglib.a \
		target/x86_64-apple-darwin/release/libriglib.a \
		-create -output rigx/lib/libriglib.a
	cd rigx && xcodebuild -configuration Release -scheme rigx -derivedDataPath build-x86_64 -arch x86_64
	cd rigx && xcodebuild -configuration Release -scheme rigx -derivedDataPath build-arm64 -arch arm64

# -------------------------------------------------------------------------

win: rig-$(VERSION).exe

rig-$(VERSION).exe: target/release/rig.exe rig.iss gsudo.exe
	find target/release -name _rig.ps1 -exec cp \{\} _rig.ps1 \;
	"C:\Program Files (x86)\Inno Setup 6\ISCC.exe" rig.iss
	cp output\mysetup.exe $@

gsudo.exe:
	curl -L https://github.com/gerardog/gsudo/releases/download/v1.0.2/gsudo.v1.0.2.zip -o gsudo.zip
	unzip gsudo.zip

# -------------------------------------------------------------------------

linux: export OPENSSL_DIR = /usr/local/
linux: export OPENSSL_INCLUDE_DIR = /usr/local/include/
linux: export OPENSSL_LIB_DIR = /usr/local/lib/
linux: export OPENSSL_STATIC = 1
linux: export DEP_OPENSSL_INCLUDE = /usr/local/include/
linux: rig-$(VERSION).tar.gz

rig-$(VERSION).tar.gz: target/release/rig
	strip -x target/release/rig
	mkdir -p build/bin
	mkdir -p build/share/bash-completion/completions
	mkdir -p build/share/zsh/site-functions
	cp target/release/rig build/bin
	find target/release/build -name _rig -exec cp \{\} build/share/zsh/site-functions \; 
	find target/release/build -name rig.bash -exec cp \{\} build/share/bash-completion/completions \; 
	tar cz -C build -f $@ bin share

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

release: rig-$(VERSION)-macOS-arm64.pkg rig-$(VERSION)-macOS-x86_64.pkg

rig-$(VERSION)-macOS-%.pkg: rig-unnotarized-%.pkg gon.hcl.in
	cat gon.hcl.in | \
		sed 's/{{VERSION}}/$(VERSION)/g' | \
		sed 's/{{ARCH}}/$*/g' > gon.hcl
	cp $< $@
	gon -log-level=warn ./gon.hcl

rig-unnotarized-%.pkg: build.stamp  distribution.xml.in
	codesign --force \
		--options runtime \
		-s 8ADFF507AE8598B1792CF89213307C52FAFF3920 \
		build-$*/usr/local/bin/rig
	pkgbuild --root build-$* \
		--identifier com.gaborcsardi.rig \
		--version $(VERSION) \
		--ownership recommended \
		rig-$*.pkg
	cat distribution.xml.in | sed "s/{{VERSION}}/$(VERSION)/g" | \
		 sed "s/{{ARCH}}/$*/g" > distribution.xml
	productbuild --distribution distribution.xml \
		--resources Resources \
		--package-path rig-$*.pkg \
		--version $(VERSION) \
		--sign "Developer ID Installer: Gabor Csardi" $@

macos-unsigned: rig-$(VERSION)-macOS-unsigned-arm64.pkg rig-$(VERSION)-macOS-unsigned-x86_64.pkg

macos-unsigned-x86_64: rig-$(VERSION)-macOS-unsigned-x86_64.pkg

macos-unsigned-arm64: rig-$(VERSION)-macOS-unsigned-arm64.pkg

rig-$(VERSION)-macOS-unsigned-%.pkg: build.stamp  distribution.xml.in
	pkgbuild --root build-$* \
		--identifier com.gaborcsardi.rig \
		--version $(VERSION) \
		--ownership recommended \
		$@
	cat distribution.xml.in | sed "s/{{VERSION}}/$(VERSION)/g" | \
		 sed "s/{{ARCH}}/$*/g" > distribution.xml

README.md: README.Rmd $(SOURCES)
	cargo build --release
	R -q -e 'rmarkdown::render("README.Rmd")'

build.stamp: target/release/rig target/x86_64-apple-darwin/release/rig
	rm -rf build-arm64 build-x86_64
	# arm64
	mkdir -p build-arm64/usr/local/bin
	mkdir -p build-arm64/usr/local/share/zsh/site-functions
	mkdir -p build-arm64/opt/homebrew/etc/bash_completion.d/
	cp target/release/rig build-arm64/usr/local/bin/
	strip -x build-arm64/usr/local/bin/rig
	find target/release/build -name _rig -exec cp \{\} build-arm64/usr/local/share/zsh/site-functions \; 
	find target/release/build -name rig.bash -exec cp \{\} build-arm64/opt/homebrew/etc/bash_completion.d \; 
	# x86_64
	mkdir -p build-x86_64/usr/local/bin
	mkdir -p build-x86_64/usr/local/share/zsh/site-functions
	mkdir -p build-x86_64/opt/homebrew/etc/bash_completion.d/
	cp target/x86_64-apple-darwin/release/rig build-x86_64/usr/local/bin/
	strip -x build-x86_64/usr/local/bin/rig
	find target/release/build -name _rig -exec cp \{\} build-x86_64/usr/local/share/zsh/site-functions \; 
	find target/release/build -name rig.bash -exec cp \{\} build-x86_64/opt/homebrew/etc/bash_completion.d \; 
	# Resources
	rm -rf Resources
	mkdir Resources
	cp README.md NEWS.md LICENSE Resources/
	touch $@

# -------------------------------------------------------------------------

.PHONY: release clean all macos win linux rigx

clean:
	rm -rf build.stamp build-* Resources *.pkg distribution.xml gon.hcl Output *.exe
