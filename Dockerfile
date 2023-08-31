
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

RUN adduser rig -D

USER rig

RUN cd && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o rust.sh && sh rust.sh -y

USER root

RUN mkdir /work

WORKDIR /work

ENV PATH="/home/rig/.cargo/bin:$PATH"

COPY entrypoint.sh /entrypoint.sh

RUN chmod +x /entrypoint.sh

ENTRYPOINT [ "sh", "/entrypoint.sh" ]
