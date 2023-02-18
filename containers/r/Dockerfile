
FROM ubuntu:22.04

ARG PAK_VERSION devel

RUN ARCH="$(arch)"; \
    if [ "$ARCH" = "aarch64" ]; then ARM64="-arm64"; else ARM64=""; fi; \
    apt-get update -y && \
    apt-get install -y curl && \
    curl -Ls https://github.com/r-lib/rig/releases/download/latest/rig-linux${ARM64}-latest.tar.gz | tar xz -C /usr/local && \
    rig add devel --without-pak && \
    rig add next --without-pak && \
    rig add release --without-pak && \
    rig add oldrel/1 --without-pak && \
    rig add oldrel/2 --without-pak && \
    rig add oldrel/3 --without-pak && \
    rig add oldrel/4 --without-pak && \
    rig default release && \
    rm -rf /tmp/rig && \
    apt-get clean

# We want to install pak into the system library, so we don't do it with rig

RUN versions="$(rig ls --json | grep '"name"' | sed 's/^.*\"name\": \"//' | sed 's/\",//')" && \
    for ver in $versions; do \
      R-${ver} -q -e \
      'install.packages("pak", lib = .Library, repos = sprintf("https://r-lib.github.io/p/pak/%s/%s/%s/%s", "devel", .Platform$pkgType, R.Version()$os, R.Version()$arch))'; \
    done

RUN apt-get update -y && \
    apt-get install -y locales && \
    apt-get clean && \
    locale-gen en_US.UTF-8 && \
    update-locale
ENV LC_COLLATE C
ENV LANG en_US.UTF-8

# Install some software that people need a lot

RUN apt-get update -y && \
    apt-get install -y git && \
    apt-get clean

WORKDIR /root

# Use new sysreqs

ENV PKG_SYSREQS=true
ENV R_PKG_SYSREQS2=true
