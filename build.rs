fn main() {
    println!("cargo:rerun-if-changed=src/gamescope/pipewire_dma.c");
    println!("cargo:rerun-if-changed=src/gamescope/pipewire_dma.h");

    let pw = pkg_config::Config::new()
        .atleast_version("0.3")
        .probe("libpipewire-0.3")
        .expect("Failed to find libpipewire-0.3 via pkg-config");

    let mut build = cc::Build::new();
    build.file("src/gamescope/pipewire_dma.c");
    build.include("src/gamescope");
    for include in pw.include_paths {
        build.include(include);
    }
    build.compile("pipewire_dma_bridge");

    println!("cargo:rustc-link-lib=pipewire-0.3");
}
