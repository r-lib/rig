ARG DISTRO=ubuntu
ARG RELEASE=mandatory
FROM ${DISTRO}:${RELEASE}

ARG RVERSION=release

COPY rig.gpg /etc/apt/trusted.gpg.d/rig.gpg

RUN echo "deb http://rig.r-pkg.org/deb rig main" > \
    /etc/apt/sources.list.d/rig.list

RUN apt update && \
    apt install r-rig && \
    apt clean

RUN rig add ${RVERSION} --without-pak && \
    rm -rf /tmp/rig && \
    apt-get clean

# We want to install pak into the system library, so we don't do it with rig

RUN for inst in $(ls /opt/R/*/lib/R/bin/INSTALL); do \
      sed -i '1s/^/#!\/bin\/bash\n/' ${inst}; \
    done; \
    versions="$(rig ls --json | grep '"name"' | sed 's/^.*\"name\": \"//' | sed 's/\",//')" && \
    for ver in $versions; do \
      R-${ver} -q -e 'source("https://pak.r-lib.org/install.R?stream=devel")'; \
    done

RUN apt-get update && \
    apt-get install -y locales && \
    apt-get clean && \
    echo "en_US.UTF-8 UTF-8" >> /etc/locale.gen \
	&& locale-gen en_US.utf8 \
	&& /usr/sbin/update-locale LANG=en_US.UTF-8
ENV LC_COLLATE C
ENV LANG en_US.UTF-8

WORKDIR /root
