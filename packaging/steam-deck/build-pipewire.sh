#!/usr/bin/env bash
set -euo pipefail
mkdir -p /tmp/michadame-pipewire
cd /tmp/michadame-pipewire
curl -fL --retry 3 https://deb.debian.org/debian/pool/main/p/pipewire/pipewire_1.6.9.orig.tar.gz -o pipewire.tar.gz
echo 'a0012db8aaca4448fd205035e07d264235e4a4fe50713085fc4a6899e738a298  pipewire.tar.gz' | sha256sum -c -
mkdir source
tar xf pipewire.tar.gz -C source --strip-components=1
meson setup build source --prefix=/opt/michadame-deps --libdir=lib \
    --buildtype=release -Dauto_features=disabled -Dtests=disabled -Dexamples=disabled \
    -Dpipewire-jack=disabled -Dpipewire-v4l2=disabled -Ddbus=disabled -Dflatpak=disabled \
    -Dsession-managers=[]
ninja -C build install
mkdir -p /opt/michadame-deps/share/licenses/pipewire
cp source/COPYING source/LICENSE* /opt/michadame-deps/share/licenses/pipewire/
cd /
rm -rf /tmp/michadame-pipewire
