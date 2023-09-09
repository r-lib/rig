#!! /bin/sh

# make sure that there is a group with $LOCAL_GID

getent group $LOCAL_GID
cat /etc/passwd
cat /etc/group
id -u
id -g

group=`getent group $LOCAL_GID | cut -f1 -d:`
if [ -z "$group" ]; then
    addgroup -g "$LOCAL_GID" rigbuild
    group=rigbuild
fi

# Recreate rigbuild user with the right UID and GID

deluser rigbuild 2>/dev/null
adduser -u $LOCAL_UID -G $group -D -H rigbuild

# We only need acceess to these and it would takes ~10s to chown all the
# files of the rust toolchain

chown rigbuild:$group /home/rigbuild/
chown rigbuild:$group /home/rigbuild/.cargo

exec su -s /bin/sh rigbuild sh -c "$*"
