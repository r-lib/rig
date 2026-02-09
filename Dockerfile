
FROM alpine:3.22

RUN apk add curl linux-headers bash gcc musl-dev g++ pkgconf make file

# install rust toolchain for 'rigbuild' user ==============================

RUN adduser rigbuild -D
USER rigbuild
RUN cd && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o rust.sh && sh rust.sh -y
USER root
ENV PATH="/home/rigbuild/.cargo/bin:$PATH"
COPY tools/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
ENTRYPOINT [ "sh", "/entrypoint.sh" ]

# this is the shared directory =============================================

RUN mkdir /work
WORKDIR /work

# packageer ===============================================================

RUN curl -LO https://github.com/goreleaser/nfpm/releases/download/v2.32.0/nfpm_2.32.0_$(arch).apk && \
    apk add --allow-untrusted nfpm*.apk && \
    rm nfpm*.apk
