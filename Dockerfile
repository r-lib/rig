
FROM alpine:3.15

RUN apk add curl linux-headers bash gcc musl-dev g++ pkgconf make file

# zlib --------------------------------------------------------------------

RUN curl -OL https://downloads.sourceforge.net/project/libpng/zlib/1.2.11/zlib-1.2.11.tar.gz
RUN tar xzf zlib-*.tar.gz && rm zlib-*.tar.gz
RUN cd zlib-* &&                                    \
    CFLAGS=-fPIC ./configure --static &&            \
    make &&                                         \
    make install

# openssl -----------------------------------------------------------------

RUN curl -O https://www.openssl.org/source/openssl-1.1.1m.tar.gz
RUN tar xzf openssl-*.tar.gz && rm openssl-*.tar.gz
RUN apk add perl linux-headers
RUN cd openssl-* &&                                 \
    CFLAGS=-fPIC ./config -fPIC no-shared &&        \
    make &&                                         \
    make install_sw &&                              \
    rm -rf /usr/local/bin/openssl                   \
       /usr/local/share/{man/doc}

# install rust toolchain for 'rigbuild' user ==============================

RUN adduser rigbuild -D
USER rigbuild
RUN cd && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o rust.sh && sh rust.sh -y
USER root
ENV PATH="/home/rigbuild/.cargo/bin:$PATH"
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
ENTRYPOINT [ "sh", "/entrypoint.sh" ]

# this is the shared directory =============================================

RUN mkdir /work
WORKDIR /work

# packageer ===============================================================

RUN curl -LO https://github.com/goreleaser/nfpm/releases/download/v2.32.0/nfpm_2.32.0_$(arch).apk && \
    apk add --allow-untrusted nfpm*.apk && \
    rm nfpm*.apk
