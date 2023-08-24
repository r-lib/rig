
cd /work

PATH=/work/tests/bats/bin:$PATH
bats --version

VERSION=$(grep "^version" /work/Cargo.toml | tr -cd '0-9.')
tar xzf /work/rig-${VERSION}.tar.gz -C /

rig --version

bats tests/test-linux.sh
