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

rm -rf /tmp/nfpm-rpm
mkdir -p /tmp/nfpm-rpm
tar xzf "$1" -C /tmp/nfpm-rpm

rm -rf /tmp/nfpm
mkdir -p /tmp/nfpm

cat <<EOF >/tmp/nfpm-rpm.yaml
name: r-rig
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
contents:
- src: /tmp/nfpm-rpm
  dst: /usr/local
EOF

nfpm package \
     -f /tmp/nfpm-rpm.yaml \
     -p rpm \
     -t /tmp/nfpm

out="`ls /tmp/nfpm`"
cp "/tmp/nfpm/$out" .
ln -sf "`basename $out`" $2
