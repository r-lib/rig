# A container image that carries nothing but the `rig` binary. It is not
# meant to be run directly; copy the binary out of it into your own image:
#
#   COPY --from=ghcr.io/r-lib/rig/base:latest /rig /usr/local/bin/rig
#
# Installs whatever version website/install.sh currently resolves to with no
# `--version` override, so it tracks the same beta-or-stable default that
# `curl -LsSf https://r-lib.github.io/rig/install.sh | sh` installs.

FROM debian:bookworm-slim AS builder
RUN apt-get update && apt-get install -y --no-install-recommends \
      curl ca-certificates
RUN curl -LsSf https://r-lib.github.io/rig/install.sh | \
      sh -s -- --no-modify-path --prefix=/opt/rig

FROM scratch
COPY --from=builder /opt/rig/bin/rig /rig
