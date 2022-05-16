
FROM alpine:3.15

COPY . rig

RUN apk add curl

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o rust.sh && sh rust.sh -y

RUN apk add linux-headers bash gcc musl-dev g++ pkgconf make

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

# build rig ---------------------------------------------------------------

RUN source $HOME/.cargo/env && cd rig && make linux

RUN mkdir out && cp rig/rig-*.tar.gz out

RUN ls -l out
