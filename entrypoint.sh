#!! /bin/sh

# There is already a rigbuild user and a rigbuild group, we
# just need to make sure that their uid and gid matches the host.
apk add shadow

user=`getent passwd $LOCAL_UID | cut -f1 -d:`
if [ -z "$user" ]; then
    usermod -u $LOCAL_UID rigbuild
    user=rigbuild
fi

group=`getent group $LOCAL_GID | cut -f1 -d:`
if [ -z "$group" ]; then
    groupmod -g ${LOCAL_GID} rigbuild
    group=rigbuild
fi

# We only need acceess to these and it would takes ~10s to chown all the
# files of the rust toolchain

chown $user:$group /home/rigbuild/
chown $user:$group /home/rigbuild/.cargo

exec su -s /bin/sh $user sh -c "$*"
