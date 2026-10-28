#!/usr/bin/env bash
set -euo pipefail
mkdir -p /tmp/michadame-sources
cd /tmp/michadame-sources
curl -fL --retry 3 https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz -o ffmpeg.tar.xz
echo '8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e  ffmpeg.tar.xz' | sha256sum -c -
curl -fL --retry 3 https://deb.debian.org/debian/pool/main/libe/libei/libei_1.5.0.orig.tar.bz2 -o libei.tar.bz2
echo 'da1fba92daccd0667bc46c3ee952d4ae8cfc6bdb4c0bb4d34df26528fb240618  libei.tar.bz2' | sha256sum -c -
tar xf ffmpeg.tar.xz
tar xf libei.tar.bz2
meson setup libei-1.5.0/build libei-1.5.0 --prefix=/opt/michadame-deps --libdir=lib \
    -Dtests=disabled -Dliboeffis=disabled -Dlibeis=disabled
ninja -C libei-1.5.0/build install
cd ffmpeg-9.0.2
./configure --prefix=/opt/michadame-deps --enable-shared --disable-static \
    --disable-autodetect --enable-gpl --enable-libx264 --enable-libmp3lame \
    --enable-vaapi --enable-libdrm --enable-libv4l2 --enable-zlib --enable-bzlib \
    --enable-lzma --enable-alsa --disable-debug --disable-doc
make -j"$(nproc)"
make install
mkdir -p /opt/michadame-deps/share/licenses/ffmpeg /opt/michadame-deps/share/licenses/libei
cp COPYING.* LICENSE.md /opt/michadame-deps/share/licenses/ffmpeg/
cp ../libei-1.5.0/COPYING /opt/michadame-deps/share/licenses/libei/
cd /
rm -rf /tmp/michadame-sources
