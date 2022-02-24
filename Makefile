
VERSION=$(shell grep "^version" Cargo.toml | tr -cd '0-9.')
SOURCES=$(wildcard src/*.rs) $(wildcard src/*.in)

all:
	@echo "Call 'make win', 'make macos' or 'make linux'"

# -------------------------------------------------------------------------

win: rim-$(VERSION).exe

rim-$(VERSION).exe: target/release/rim.exe rim.iss gsudo.exe
	find target/release -name _rim.ps1 -exec cp \{\} _rim.ps1 \;
	"C:\Program Files (x86)\Inno Setup 6\ISCC.exe" rim.iss
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
linux: rim-$(VERSION).tar.gz

rim-$(VERSION).tar.gz: target/release/rim
	strip -x target/release/rim
	mkdir -p build/bin
	mkdir -p build/share/bash-completion/completions
	mkdir -p build/share/zsh/site-functions
	cp target/release/rim build/bin
	find target/release/build -name _rim -exec cp \{\} build/share/zsh/site-functions \; 
	find target/release/build -name rim.bash -exec cp \{\} build/share/bash-completion/completions \; 
	tar cz -C build -f $@ bin share

# -------------------------------------------------------------------------

macos: release

target/release/rim.exe: $(SOURCES)
	rm -rf target/release/build/rim-*
	cargo build --release

target/release/rim: $(SOURCES)
	rm -rf target/release/build/rim-*
	cargo build --release

target/x86_64-apple-darwin/release/rim: $(SOURCES)
	rm -rf target/x86_64-apple-darwin/release/build/rim-*
	cargo build --target x86_64-apple-darwin --release

release: rim-$(VERSION)-macOS-arm64.pkg rim-$(VERSION)-macOS-x86_64.pkg

rim-$(VERSION)-macOS-%.pkg: rim-unnotarized-%.pkg gon.hcl.in
	cat gon.hcl.in | \
		sed 's/{{VERSION}}/$(VERSION)/g' | \
		sed 's/{{ARCH}}/$*/g' > gon.hcl
	cp $< $@
	gon -log-level=warn ./gon.hcl

rim-unnotarized-%.pkg: build.stamp  distribution.xml.in
	codesign --force \
		--options runtime \
		-s 8ADFF507AE8598B1792CF89213307C52FAFF3920 \
		build-$*/usr/local/bin/rim
	pkgbuild --root build-$* \
		--identifier com.gaborcsardi.rim \
		--version $(VERSION) \
		--ownership recommended \
		rim-$*.pkg
	cat distribution.xml.in | sed "s/{{VERSION}}/$(VERSION)/g" | \
		 sed "s/{{ARCH}}/$*/g" > distribution.xml
	productbuild --distribution distribution.xml \
		--resources Resources \
		--package-path rim-$*.pkg \
		--version $(VERSION) \
		--sign "Developer ID Installer: Gabor Csardi" $@

README.md: README.Rmd $(SOURCES)
	cargo build --release
	R -q -e 'rmarkdown::render("README.Rmd")'

build.stamp: target/release/rim target/x86_64-apple-darwin/release/rim README.md
	rm -rf build-arm64 build-x86_64
	# arm64
	mkdir -p build-arm64/usr/local/bin
	mkdir -p build-arm64/usr/local/share/zsh/site-functions
	mkdir -p build-arm64/opt/homebrew/etc/bash_completion.d/
	cp target/release/rim build-arm64/usr/local/bin/
	strip -x build-arm64/usr/local/bin/rim
	find target/release/build -name _rim -exec cp \{\} build-arm64/usr/local/share/zsh/site-functions \; 
	find target/release/build -name rim.bash -exec cp \{\} build-arm64/opt/homebrew/etc/bash_completion.d \; 
	# x86_64
	mkdir -p build-x86_64/usr/local/bin
	mkdir -p build-x86_64/usr/local/share/zsh/site-functions
	mkdir -p build-x86_64/opt/homebrew/etc/bash_completion.d/
	cp target/x86_64-apple-darwin/release/rim build-x86_64/usr/local/bin/
	strip -x build-x86_64/usr/local/bin/rim
	find target/release/build -name _rim -exec cp \{\} build-x86_64/usr/local/share/zsh/site-functions \; 
	find target/release/build -name rim.bash -exec cp \{\} build-x86_64/opt/homebrew/etc/bash_completion.d \; 
	# Resources
	rm -rf Resources
	mkdir Resources
	cp README.md NEWS.md LICENSE Resources/
	touch $@

# -------------------------------------------------------------------------

.PHONY: release clean all macos win linux

clean:
	rm -rf build.stamp build-* Resources *.pkg distribution.xml gon.hcl Output *.exe
