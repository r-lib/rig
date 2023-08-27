
cd /work

PATH=/work/tests/bats/bin:$PATH
bats --version

VERSION=$(grep "^version" /work/Cargo.toml | tr -cd '0-9.')
# We can't use the built tar.gz, because opensuse does not have tar (!)
cp -r /work/build/* /usr/local/

export SSL_CERT_FILE=/usr/local/share/rig/cacert.pem
rig --version

bats --print-output-on-failure tests/test-linux.sh "$@"
