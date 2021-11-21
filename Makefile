
# This is currently macOS specific, and works best on arm64 macOS.
# We'll update it to support Windows once Windows support is added.

VERSION=$(shell grep "^version" Cargo.toml | tr -cd '0-9.')
SOURCES=$(wildcard src/*.rs)
ARCH=arm64

target/debug/rim: $(SOURCES)
	cargo build

target/release/rim: $(SOURCES)
	cargo build --release

release: rim-$(ARCH)-$(VERSION)-macOS.pkg

rim-$(ARCH)-$(VERSION)-macOS.pkg: rim-unnotarized-$(ARCH).pkg
	cat gon.hcl.in | \
		sed 's/{{VERSION}}/$(VERSION)/g' | \
		sed 's/{{ARCH}}/$(ARCH)/g' > gon.hcl
	cp $< $@
	gon -log-level=warn ./gon.hcl

rim-unnotarized-$(ARCH).pkg: build.stamp
	codesign --force \
		--options runtime \
		-s 8ADFF507AE8598B1792CF89213307C52FAFF3920 \
		build-amd64/usr/local/bin/rim
	pkgbuild --root build-amd64 \
		--identifier com.gaborcsardi.rim \
		--version $(VERSION) \
		--ownership recommended \
		rim-$(ARCH).pkg
	productbuild --distribution distribution.xml \
		--resources Resources \
		--package-path rim-$(ARCH).pkg \
		--version $(VERSION) \
		--sign "Developer ID Installer: Gabor Csardi" $@

build.stamp: target/release/rim distribution.xml
	rm -rf build-amd64 build-x86_64
	mkdir -p build-amd64/usr/local/bin
	cp target/release/rim build-amd64/usr/local/bin/
	strip -x build-amd64/usr/local/bin/rim
	rm -rf Resources
	mkdir Resources
	cp README.md NEWS.md LICENSE Resources/
	touch $@

distribution.xml: distribution.xml.in
	cat $< | sed "s/{{VERSION}}/$(VERSION)/g" | \
		 sed "s/{{ARCH}}/$(ARCH)/g" > $@

.PHONY: release clean

clean:
	rm -rf build* Resources *.pkg distribution.xml gon.hcl
