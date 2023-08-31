#!! /bin/sh

group=`getent group $LOCAL_GID | cut -f1 -d:`
if [ -z "$group" ]; then
    addgroup -g "$LOCAL_GID" rig
    group=rig
fi

deluser rig 2>/dev/null
adduser -u $LOCAL_UID -G $group -D -H rig
chown rig:$group /home/rig/
chown rig:$group /home/rig/.cargo

exec su -s /bin/sh rig
