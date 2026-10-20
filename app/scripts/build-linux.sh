#!/usr/bin/env bash
# Builds the Linux packages in Docker, from WSL or any Linux box:
#   bash app/scripts/build-linux.sh [deck|desktop|all]   (default: deck)
#   deck    — Steam Deck edition, console mode only: AppImage
#   desktop — the regular app (desktop + console mode): deb + rpm
# The sources are copied into a Docker volume (fast builds, no root-owned files
# in the repo); cargo / pnpm caches live in volumes too. Result: target/linux/.
set -euo pipefail

WHAT="${1:-deck}"
case "$WHAT" in deck | desktop | all) ;; *) echo "usage: $0 [deck|desktop|all]" >&2; exit 2 ;; esac

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
IMAGE=hyper-linux-build:1

docker build -t "$IMAGE" "$ROOT/app/scripts/linux"
mkdir -p "$ROOT/target/linux"

docker run --rm \
  -e WHAT="$WHAT" \
  -v "$ROOT:/src:ro" \
  -v hyper-linux-work:/work \
  -v hyper-linux-cargo:/usr/local/cargo/registry \
  -v hyper-linux-pnpm:/pnpm-store \
  -v "$ROOT/target/linux:/out" \
  "$IMAGE" bash -euo pipefail -c '
    rsync -a --delete \
      --exclude .git --exclude node_modules --exclude /target \
      --exclude /app/build \
      /src/ /work/src/
    cd /work/src/app
    pnpm config set store-dir /pnpm-store
    pnpm install --frozen-lockfile
    BUNDLE=/work/src/target/release/bundle
    if [ "$WHAT" = deck ] || [ "$WHAT" = all ]; then
      rm -rf "$BUNDLE/appimage" /out/*.AppImage
      pnpm app:build:deck
      for f in "$BUNDLE"/appimage/*.AppImage; do
        cp "$f" "/out/$(basename "$f" | sed "s/^Hyper_/Hyper-Deck_/")"
      done
    fi
    if [ "$WHAT" = desktop ] || [ "$WHAT" = all ]; then
      rm -rf "$BUNDLE/deb" "$BUNDLE/rpm" /out/*.deb /out/*.rpm
      pnpm app:build
      cp "$BUNDLE"/deb/*.deb "$BUNDLE"/rpm/*.rpm /out/
    fi
    ls -lh /out
  '
