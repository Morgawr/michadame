# Build on glibc 2.35; the resulting application runs natively on SteamOS.
FROM rust@sha256:77fac8b98f9f46062bb680b6d25d5bcaabfc400143952ebc572e924bcbedc3fa AS rust
FROM ubuntu@sha256:e0f80c829509ef4df9e76bd8944eb7b8ec25cbb379b112a7f9448b2c77a77c9b
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential ca-certificates curl xz-utils bzip2 pkg-config clang libclang-dev \
    cmake ninja-build python3 python3-pip python3-jinja2 python3-attr patchelf file \
    binutils git nasm libudev-dev libusb-1.0-0-dev libasound2-dev libasound2-plugins \
    libpipewire-0.3-dev libspa-0.2-dev libx11-dev libxfixes-dev libxkbcommon-dev \
    libwayland-dev libegl1-mesa-dev libgl1-mesa-dev libva-dev libdrm-dev libpulse-dev \
    libssl-dev libx264-dev libmp3lame-dev libv4l-dev zlib1g-dev libbz2-dev liblzma-dev \
    libsdl2-dev pipewire-bin pulseaudio-utils libxml2-dev libevdev-dev \
    xvfb xauth mesa-utils libgl1-mesa-dri gstreamer1.0-tools \
    gstreamer1.0-plugins-base gstreamer1.0-pipewire \
    && python3 -m pip install --no-cache-dir meson==1.9.1 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=rust /usr/local/rustup /usr/local/rustup
COPY --from=rust /usr/local/cargo /usr/local/cargo
ENV RUSTUP_HOME=/usr/local/rustup PATH=/usr/local/cargo/bin:$PATH
COPY build-deps.sh /opt/build-deps.sh
RUN bash /opt/build-deps.sh
COPY build-pipewire.sh /opt/build-pipewire.sh
RUN bash /opt/build-pipewire.sh
ENV PKG_CONFIG_PATH=/opt/michadame-deps/lib/pkgconfig LD_LIBRARY_PATH=/opt/michadame-deps/lib
WORKDIR /work
