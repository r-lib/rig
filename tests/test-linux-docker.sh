
set -e
source /etc/os-release
if [[ "$ID" == "rhel" ]]; then
  if [[ "$VERSION_ID" =~ ^7 ]]; then
    REDHAT_ORG=${REDHAT_ORG_RHEL7}
    REDHAT_ACTIVATION_KEY=${REDHAT_ACTIVATION_KEY_RHEL7}
  elif [[ "$VERSION_ID" =~ ^8 ]]; then
    REDHAT_ORG=${REDHAT_ORG_RHEL8}
    REDHAT_ACTIVATION_KEY=${REDHAT_ACTIVATION_KEY_RHEL8}
  elif [[ "$VERSION_ID" =~ ^9 ]]; then
    REDHAT_ORG=${REDHAT_ORG_RHEL9}
    REDHAT_ACTIVATION_KEY=${REDHAT_ACTIVATION_KEY_RHEL9}
  fi
  trap "subscription-manager unregister" EXIT
  subscription-manager register --org ${REDHAT_ORG} \
    --activationkey  ${REDHAT_ACTIVATION_KEY}
fi

cd /work

PATH=/work/tests/bats/bin:$PATH
bats --version

VERSION=$(grep "^version" /work/Cargo.toml | tr -cd '0-9.')
# We can't use the built tar.gz, because opensuse does not have tar (!)
cp -r /work/build/* /usr/local/

export SSL_CERT_FILE=/usr/local/share/rig/cacert.pem
rig --version

bats --print-output-on-failure tests/test-linux.sh "$@"
