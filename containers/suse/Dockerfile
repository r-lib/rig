ARG RELEASE=mandatory
FROM opensuse/leap:${RELEASE}

ARG RVERSION=release

RUN zypper install -y --allow-unsigned-rpm \
    https://github.com/r-lib/rig/releases/download/latest/r-rig-latest-1.$(arch).rpm

RUN rig add ${RVERSION} --without-pak

# ------------------------------------------------------------------------------------
# Install pak

RUN R -q -e 'install.packages("pak", repos = sprintf("https://r-lib.github.io/p/pak/%s/%s/%s/%s", "devel", .Platform$pkgType, R.Version()$os, R.Version()$arch), lib = .Library)'

# ------------------------------------------------------------------------------------
# Locale

ENV LC_ALL en_US.UTF-8
ENV LANG en_US.UTF-8

WORKDIR /root
