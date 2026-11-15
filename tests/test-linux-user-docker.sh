# Runs tests/test-linux-user.sh inside a stock distro container, as a regular
# (non-root) user, with rig installed into that user's ~/.local, the way the
# user-mode install instructions say. No Red Hat subscription is needed.
#
# As root, we first install the few system packages that the portable R
# builds need from the host, and that the stock images may lack:
# - `which`: the utils package calls it when it is loaded. rig warns if it
#   is missing and asks the user to install it.
# - libexpat: needed by the bundled fontconfig/cairo, for png() etc.
# - ca-certificates: rig and R use the system CA bundle.
# - util-linux: only to switch to the test user, see below.

set -e

cd /work

if command -v apt-get >/dev/null 2>&1; then
    apt-get update
    apt-get install -y ca-certificates debianutils libexpat1
elif command -v zypper >/dev/null 2>&1; then
    zypper --non-interactive install ca-certificates which libexpat1 util-linux
elif command -v dnf >/dev/null 2>&1; then
    dnf install -y ca-certificates which expat util-linux
elif command -v microdnf >/dev/null 2>&1; then
    microdnf install -y ca-certificates which expat util-linux
elif command -v yum >/dev/null 2>&1; then
    yum install -y ca-certificates which expat util-linux
fi

TEST_USER=rigtest
TEST_HOME=/home/$TEST_USER

# Create the test user. Some minimal images do not have `useradd`, so fall
# back to editing /etc/passwd directly.
if ! id "$TEST_USER" >/dev/null 2>&1; then
    if command -v useradd >/dev/null 2>&1; then
        useradd -m -d "$TEST_HOME" -s /bin/bash "$TEST_USER"
    else
        echo "$TEST_USER:x:1500:1500::$TEST_HOME:/bin/bash" >> /etc/passwd
        echo "$TEST_USER:x:1500:" >> /etc/group
        mkdir -p "$TEST_HOME"
        chown 1500:1500 "$TEST_HOME"
    fi
fi

# Install rig into ~/.local of the test user. We can't use the built tar.gz,
# because opensuse does not have tar (!)
mkdir -p "$TEST_HOME/.local"
cp -r /work/build/* "$TEST_HOME/.local/"
chown -R "$TEST_USER": "$TEST_HOME/.local"

# The tests write their temporary files and bats output here.
mkdir -p "$TEST_HOME/tmp"
chown "$TEST_USER": "$TEST_HOME/tmp"

ENV=(
    "HOME=$TEST_HOME"
    "USER=$TEST_USER"
    "LOGNAME=$TEST_USER"
    "TERM=${TERM:-dumb}"
    "TMPDIR=$TEST_HOME/tmp"
    "PATH=$TEST_HOME/.local/bin:/work/tests/bats/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
    "RIG_MODE=user"
    "RUST_BACKTRACE=${RUST_BACKTRACE:-1}"
)

CMD=(bats --print-output-on-failure tests/test-linux-user.sh "$@")

# Switch to the test user. `runuser` and `setpriv` are part of util-linux,
# `su` is the last resort.
if command -v runuser >/dev/null 2>&1; then
    exec runuser -u "$TEST_USER" -- env -i "${ENV[@]}" "${CMD[@]}"
elif command -v setpriv >/dev/null 2>&1; then
    exec setpriv --reuid="$TEST_USER" --regid="$(id -g "$TEST_USER")" \
        --init-groups -- env -i "${ENV[@]}" "${CMD[@]}"
else
    exec su "$TEST_USER" -s /bin/bash -c "$(printf '%q ' env -i "${ENV[@]}" "${CMD[@]}")"
fi
