# Build image for the Linux (Steam Deck) AppImage. Debian 12 (glibc 2.36):
# the result also runs on older desktops (MX Linux 23 / Debian 12) and on SteamOS.
# Everything is pinned: the base image by digest, Node by SHA-256, pnpm by SHA-512.
FROM rust:1.99.0-bookworm@sha256:114c7a4425406451c2866b6aafe69fe29b1b298832db1277d411ac73c82d04d6

# Tauri 2 build dependencies (https://v2.tauri.app/start/prerequisites/#linux)
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      libwebkit2gtk-4.1-dev libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev \
      libudev-dev \
      build-essential curl wget file patchelf xz-utils rsync \
 && rm -rf /var/lib/apt/lists/*

ARG NODE_VERSION=22.23.3
ARG NODE_SHA256=df450af89261115ef9f9e3830c3eeb2cc9213b63c720b1af623cb5dcbe2e02de
RUN curl -fsSLo /tmp/node.tar.xz "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz" \
 && echo "${NODE_SHA256}  /tmp/node.tar.xz" | sha256sum -c - \
 && tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 \
 && rm /tmp/node.tar.xz

# Same version + hash as `packageManager` in app/package.json (keep both in sync):
# corepack checks the tarball against it, here and again when pnpm runs in app/.
# The hash is the npm registry's dist.integrity of that version, base64 -> hex.
ARG PNPM_VERSION=10.28.0
ARG PNPM_SHA512=05df71d1421f21399e053fde567cea34d446fa02c76571441bfc1c7956e98e363088982d940465fd34480d4d90a0668bc12362f8aa88000a64e83d0b0e47be48
RUN corepack enable && corepack prepare "pnpm@${PNPM_VERSION}+sha512.${PNPM_SHA512}" --activate

# linuxdeploy (used by the AppImage bundler) is itself an AppImage; there is no
# FUSE inside the container.
ENV APPIMAGE_EXTRACT_AND_RUN=1 \
    CI=true
