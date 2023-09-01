#! /bin/bash

set -e

if [ "$#" != 2 ]; then
    echo "Usaga: $1 <input-tar-gz> <output-package>"
    exit 1
fi

arch="`arch`"
if [ "$arch" == "aarch64" ]; then
    arch=arm64
elif [ "$arch" == "x86_64" ]; then
    arch=amd64
else
    echo "Unsupported architecture: $arch"
    exit 2
fi

rm -rf /tmp/nfpm-deb
mkdir -p /tmp/nfpm-deb
tar xzf "$1" -C /tmp/nfpm-deb

rm -rf /tmp/nfpm
mkdir -p /tmp/nfpm

cat <<EOF >/tmp/nfpm-deb.yaml
name: rig
version: ${VERSION}
release: 1
section: universe/math
priority: normal
arch: "${arch}"
maintainer: Gabor Csardi <csardi.gabor@gmail.com>
description: |
  The R Installation Manager
vendor: Gabor Csardi
homepage: https://github.com/r-lib/rig
license: MIT
deb:
  fields:
    Bugs: https://github.com/r-lib/rig/issues
contents:
- src: /tmp/nfpm-deb
  dst: /usr/local
EOF

nfpm package \
     -f /tmp/nfpm-deb.yaml \
     -p deb \
     -t /tmp/nfpm

out="`ls /tmp/nfpm`"
cp "/tmp/nfpm/$out" .
ln -sf "`basename $out`" $2
