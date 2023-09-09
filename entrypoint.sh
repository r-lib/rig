#!! /bin/sh

# make sure that there is a group with $LOCAL_GID

group=`getent group $LOCAL_GID | cut -f1 -d:`
if [ -z "$group" ]; then
    # if there is a 'rigbuild' group, recreate it with the right gid
    delgroup rigbuild 2>/dev/null || true
    addgroup -g "$LOCAL_GID" rigbuild
    group=rigbuild
fi

# Recreate rigbuild user with the right UID and GID

deluser rigbuild 2>/dev/null || true
adduser -u $LOCAL_UID -G $group -D -H rigbuild

# We only need acceess to these and it would takes ~10s to chown all the
# files of the rust toolchain

chown rigbuild:$group /home/rigbuild/
chown rigbuild:$group /home/rigbuild/.cargo

exec su -s /bin/sh rigbuild sh -c "$*"
