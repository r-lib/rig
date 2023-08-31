#!! /bin/sh

# make sure that there is a group with $LOCAL_GID

group=`getent group $LOCAL_GID | cut -f1 -d:`
if [ -z "$group" ]; then
    addgroup -g "$LOCAL_GID" rig
    group=rig
fi

# Recreate rig user with the right UID and GID

deluser rig 2>/dev/null
adduser -u $LOCAL_UID -G $group -D -H rig

# We only need acceess to these and it would takes ~10s to chown all the
# files of the rust toolchain

chown rig:$group /home/rig/
chown rig:$group /home/rig/.cargo

exec su -s /bin/sh rig sh -c "$*"
