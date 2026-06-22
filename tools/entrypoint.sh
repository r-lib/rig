#! /bin/sh

set -x

# Run as the host user's UID/GID so files written into the bind-mounted repo
# (Cargo.lock, the make-linux artifacts) and the named volumes stay owned by
# the host user instead of root. We create a matching 'rig' user/group so tools
# that look the UID up (getpwuid) find a name and a home, and so
# `setpriv --init-groups` can resolve the user.
#
# The rust toolchain lives in /opt/rust (see the Dockerfiles), root-owned and
# world-readable, so it belongs to no user; this user only needs to write to
# its own home, the build tree (/work/target) and the cargo cache (/cargo).
#
# Alpine ships busybox addgroup/adduser; Ubuntu's adduser is a fussy perl
# wrapper, so we use the low-level groupadd/useradd there. The group is created
# first so the user can be assigned to it.

home=/home/rig

if [ -f /etc/alpine-release ]; then
    add_group() { addgroup -g "$LOCAL_GID" rig; }
    add_user() { adduser -D -h "$home" -G "$group" -u "$LOCAL_UID" rig; }
else
    add_group() { groupadd -g "$LOCAL_GID" rig; }
    add_user() { useradd -m -d "$home" -g "$LOCAL_GID" -u "$LOCAL_UID" rig; }
fi

group=`getent group $LOCAL_GID | cut -f1 -d:`
if [ -z "$group" ]; then
    add_group
    group=rig
fi

user=`getent passwd $LOCAL_UID | cut -f1 -d:`
if [ -z "$user" ]; then
    add_user
    user=rig
fi

# Hand the user the dirs it must write to. /work/target and /cargo are named
# volumes mounted as root; an empty volume only needs its top dir chowned,
# cargo and the build create their contents as the build user afterwards.
for d in "$home" /work/target /cargo; do
    [ -d "$d" ] && chown "$LOCAL_UID:$LOCAL_GID" "$d"
done

# Drop privileges with setpriv, which execs (no fork) so the final command
# becomes the session leader and owns the controlling terminal. Using su here
# instead forks bash as a grandchild, which then cannot set the terminal
# process group ("no job control in this shell"). The inner `exec` likewise
# lets bash replace the shell rather than run as its child. The `sh -lc` login
# shell sources /etc/profile, which resets PATH and drops /opt/rust/cargo/bin, so
# we prepend cargo's bin *inside* the -c script (after profile has run) rather
# than via env (which profile would then clobber).
exec setpriv --reuid "$LOCAL_UID" --regid "$LOCAL_GID" --init-groups \
    env HOME="$home" \
    sh -lc 'export PATH="/opt/rust/cargo/bin:$PATH"; cd /work && exec "$@"' sh "$@"
