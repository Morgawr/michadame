fn main() {
    println!("cargo:rerun-if-changed=src/gamescope/pipewire_dma.c");
    println!("cargo:rerun-if-changed=src/gamescope/pipewire_dma.h");
    println!("cargo:rerun-if-changed=src/gamescope/x11_cursor.c");
    println!("cargo:rerun-if-changed=src/gamescope/x11_cursor.h");

    let pw = pkg_config::Config::new()
        .atleast_version("0.3")
        .probe("libpipewire-0.3")
        .expect("Failed to find libpipewire-0.3 via pkg-config");

    pkg_config::Config::new()
        .probe("x11")
        .expect("Failed to find libX11 via pkg-config");

    pkg_config::Config::new()
        .probe("xfixes")
        .expect("Failed to find libXfixes via pkg-config");

    let mut build = cc::Build::new();
    build.file("src/gamescope/pipewire_dma.c");
    build.file("src/gamescope/x11_cursor.c");
    build.include("src/gamescope");
    for include in pw.include_paths {
        build.include(include);
    }
    build.compile("pipewire_dma_bridge");

    println!("cargo:rustc-link-lib=pipewire-0.3");
    println!("cargo:rustc-link-lib=X11");
    println!("cargo:rustc-link-lib=Xfixes");
}
