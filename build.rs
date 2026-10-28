fn main() {
    println!("cargo:rerun-if-changed=src/steam_deck/window.c");
    println!("cargo:rerun-if-changed=src/steam_deck/capture.c");
    println!("cargo:rerun-if-changed=src/gamescope/pipewire_dma.c");
    println!("cargo:rerun-if-changed=src/gamescope/pipewire_dma.h");
    println!("cargo:rerun-if-changed=src/gamescope/x11_cursor.c");
    println!("cargo:rerun-if-changed=src/gamescope/x11_cursor.h");

    let pw = pkg_config::Config::new()
        .atleast_version("0.3")
        .probe("libpipewire-0.3")
        .expect("Failed to find libpipewire-0.3 via pkg-config");

    let x11 = pkg_config::Config::new()
        .probe("x11")
        .expect("Failed to find libX11 via pkg-config");

    let xfixes = pkg_config::Config::new()
        .probe("xfixes")
        .expect("Failed to find libXfixes via pkg-config");

    let mut build = cc::Build::new();
    build.file("src/gamescope/pipewire_dma.c");
    build.file("src/gamescope/x11_cursor.c");
    build.file("src/steam_deck/window.c");
    build.file("src/steam_deck/capture.c");
    build.include("src/gamescope");
    for include in &pw.include_paths {
        build.include(include);
        let version_header = include.join("pipewire/version.h");
        if version_header.is_file() {
            println!("cargo:rerun-if-changed={}", version_header.display());
        }
    }
    for include in &x11.include_paths {
        build.include(include);
    }
    for include in &xfixes.include_paths {
        build.include(include);
    }
    build.compile("pipewire_dma_bridge");

    println!("cargo:rustc-link-lib=pipewire-0.3");
    println!("cargo:rustc-link-lib=X11");
    println!("cargo:rustc-link-lib=Xfixes");
}
