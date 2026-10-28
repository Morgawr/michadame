#!/usr/bin/env bash
# Build in a pinned Ubuntu 22.04 container, run natively on the Deck.
set -euo pipefail
repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_dir"
builder_image=michadame-steam-deck-builder:ubuntu22
docker build -t "$builder_image" packaging/steam-deck
mkdir -p target/steam-deck-cache/cargo target/steam-deck
docker run --rm --user "$(id -u):$(id -g)" \
    -v "$repo_dir:/work" -w /work \
    -e CARGO_HOME=/work/target/steam-deck-cache/cargo \
    -e CARGO_TARGET_DIR=/work/target/steam-deck-cache/build \
    -e CARGO_INCREMENTAL=0 \
    "$builder_image" bash -c '
        set -euo pipefail
        cargo build --locked --release
        python3 packaging/steam-deck/assemble.py /work/target/steam-deck/michadame-steam-deck
    '
tar -C target/steam-deck -czf target/steam-deck/michadame-steam-deck-x86_64.tar.gz michadame-steam-deck
(
    cd target/steam-deck
    sha256sum michadame-steam-deck-x86_64.tar.gz > michadame-steam-deck-x86_64.tar.gz.sha256
)
printf 'Built %s/target/steam-deck/michadame-steam-deck-x86_64.tar.gz\n' "$repo_dir"
