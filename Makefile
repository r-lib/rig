
# This is currently macOS specific, and works best on arm64 macOS.
# We'll update it to support Windows once Windows support is added.

VERSION=$(shell grep "^version" Cargo.toml | tr -cd '0-9.')
SOURCES=$(wildcard src/*.rs)

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
		build-arm64/usr/local/bin/rim
	pkgbuild --root build-arm64 \
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

build.stamp: target/release/rim target/x86_64-apple-darwin/release/rim
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

.PHONY: release clean

clean:
	rm -rf build.stamp build-* Resources *.pkg distribution.xml gon.hcl
