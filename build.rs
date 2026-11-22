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

    let xcb = pkg_config::Config::new()
        .probe("xcb")
        .expect("Failed to find libxcb via pkg-config");

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
    for include in &xcb.include_paths {
        build.include(include);
    }
    for include in &xfixes.include_paths {
        build.include(include);
    }
    build.compile("pipewire_dma_bridge");

    println!("cargo:rustc-link-lib=pipewire-0.3");
    println!("cargo:rustc-link-lib=X11");
    println!("cargo:rustc-link-lib=xcb");
    println!("cargo:rustc-link-lib=Xfixes");

    #[cfg(feature = "ps5-remote-play")]
    build_ps5_chiaki_bridge();
}

#[cfg(feature = "ps5-remote-play")]
fn build_ps5_chiaki_bridge() {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const CHIAKI_REPO: &str = "https://github.com/streetpea/chiaki-ng.git";
    const CHIAKI_TAG: &str = "v1.10.1";
    const CHIAKI_COMMIT: &str = "f30da0aaeb96fc0026cc03683153f682352e7d7c";

    println!("cargo:rerun-if-changed=src/ps5/chiaki_bridge.c");
    println!("cargo:rerun-if-changed=src/ps5/chiaki_bridge.h");
    println!("cargo:rerun-if-env-changed=CHIAKI_NG_SRC_DIR");

    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let cache_root = manifest_dir.join("target").join("chiaki-ng-cache");
    let src_dir = match std::env::var("CHIAKI_NG_SRC_DIR") {
        Ok(custom) if !custom.trim().is_empty() => PathBuf::from(custom),
        _ => cache_root.join("src"),
    };
    let build_dir = cache_root.join("build");
    let stamp_path = build_dir.join(".chiaki_commit_stamp");
    let libchiaki_a = build_dir.join("lib").join("libchiaki.a");

    let system_curl_has_wss = Command::new("curl-config")
        .arg("--protocols")
        .output()
        .ok()
        .map(|o| {
            o.status.success()
                && String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .any(|line| line.trim().eq_ignore_ascii_case("WSS"))
        })
        .unwrap_or(false);

    let desired_stamp = format!("{CHIAKI_COMMIT}:sys_curl={system_curl_has_wss}");
    let up_to_date = libchiaki_a.is_file()
        && std::fs::read_to_string(&stamp_path)
            .map(|s| s.trim() == desired_stamp)
            .unwrap_or(false);

    if !up_to_date {
        ensure_chiaki_source(
            &src_dir,
            CHIAKI_REPO,
            CHIAKI_TAG,
            CHIAKI_COMMIT,
            !system_curl_has_wss,
        );
        configure_and_build_chiaki(&src_dir, &build_dir, system_curl_has_wss);
        std::fs::write(&stamp_path, &desired_stamp).expect("Failed to write libchiaki cache stamp");
    }

    let sdl2 = pkg_config::Config::new()
        .probe("sdl2")
        .expect("Failed to find sdl2 via pkg-config (required for ps5-remote-play feature)");
    let _opus = pkg_config::Config::new()
        .probe("opus")
        .expect("Failed to find opus via pkg-config (required for ps5-remote-play feature)");
    let _openssl = pkg_config::Config::new()
        .probe("openssl")
        .expect("Failed to find openssl via pkg-config (required for ps5-remote-play feature)");
    let _libevent = pkg_config::Config::new()
        .probe("libevent")
        .expect("Failed to find libevent via pkg-config (required for ps5-remote-play feature)");
    let _jsonc = pkg_config::Config::new()
        .probe("json-c")
        .expect("Failed to find json-c via pkg-config (required for ps5-remote-play feature)");
    let _miniupnpc = pkg_config::Config::new()
        .probe("miniupnpc")
        .expect("Failed to find miniupnpc via pkg-config (required for ps5-remote-play feature)");
    if system_curl_has_wss {
        let _curl = pkg_config::Config::new()
            .probe("libcurl")
            .expect("Failed to find libcurl via pkg-config (required for ps5-remote-play feature)");
    }

    let mut bridge = cc::Build::new();
    bridge.file("src/ps5/chiaki_bridge.c");
    bridge.include("src/ps5");
    bridge.include(src_dir.join("lib").join("include"));
    bridge.include(build_dir.join("lib").join("include"));
    bridge.include(src_dir.join("third-party").join("nanopb"));
    for include in &sdl2.include_paths {
        bridge.include(include);
    }
    bridge.compile("michadame_ps5_bridge");

    emit_chiaki_link_flags(&build_dir, system_curl_has_wss);

    fn run_checked(cmd: &mut Command, context: &str) {
        let status = cmd
            .status()
            .unwrap_or_else(|e| panic!("Failed to execute {context}: {e}"));
        if !status.success() {
            panic!("{context} exited with status {status}");
        }
    }

    fn ensure_chiaki_source(
        src_dir: &Path,
        repo: &str,
        tag: &str,
        commit: &str,
        need_curl_submodule: bool,
    ) {
        if !src_dir.join("CMakeLists.txt").is_file() {
            if let Some(parent) = src_dir.parent() {
                std::fs::create_dir_all(parent).expect("Failed to create chiaki-ng cache dir");
            }
            let mut clone = Command::new("git");
            clone
                .arg("clone")
                .arg("--depth")
                .arg("1")
                .arg("--branch")
                .arg(tag)
                .arg(repo)
                .arg(src_dir);
            run_checked(&mut clone, "git clone chiaki-ng");
        }

        // Verify pinned commit matches (or fetch it explicitly)
        let current_head = Command::new("git")
            .arg("-C")
            .arg(src_dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if current_head != commit {
            let mut fetch = Command::new("git");
            fetch
                .arg("-C")
                .arg(src_dir)
                .args(["fetch", "--depth", "1", "origin", commit]);
            run_checked(&mut fetch, "git fetch pinned chiaki-ng commit");

            let mut checkout = Command::new("git");
            checkout
                .arg("-C")
                .arg(src_dir)
                .args(["checkout", "--detach", commit]);
            run_checked(&mut checkout, "git checkout pinned chiaki-ng commit");
        }

        let mut submodules = vec![
            "third-party/gf-complete",
            "third-party/jerasure",
            "third-party/nanopb",
        ];
        if need_curl_submodule {
            submodules.push("third-party/curl");
        }
        let mut sub_cmd = Command::new("git");
        sub_cmd
            .arg("-C")
            .arg(src_dir)
            .args(["submodule", "update", "--init", "--depth", "1"]);
        for sm in submodules {
            sub_cmd.arg(sm);
        }
        run_checked(&mut sub_cmd, "git submodule update for libchiaki");
    }

    fn configure_and_build_chiaki(src_dir: &Path, build_dir: &Path, system_curl_has_wss: bool) {
        std::fs::create_dir_all(build_dir).expect("Failed to create libchiaki build dir");

        let has_ninja = Command::new("ninja")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        let mut cfg = Command::new("cmake");
        cfg.arg("-S").arg(src_dir).arg("-B").arg(build_dir);
        if has_ninja {
            cfg.args(["-G", "Ninja"]);
        }
        cfg.args([
            "-DCMAKE_BUILD_TYPE=Release",
            "-DCMAKE_POSITION_INDEPENDENT_CODE=ON",
            "-DCHIAKI_ENABLE_GUI=OFF",
            "-DCHIAKI_ENABLE_CLI=OFF",
            "-DCHIAKI_ENABLE_TESTS=OFF",
            "-DCHIAKI_ENABLE_ANDROID=OFF",
            "-DCHIAKI_ENABLE_BOREALIS=OFF",
            "-DCHIAKI_ENABLE_SETSU=OFF",
            "-DCHIAKI_ENABLE_STEAMDECK_NATIVE=OFF",
            "-DCHIAKI_ENABLE_STEAM_SHORTCUT=OFF",
            "-DCHIAKI_ENABLE_FFMPEG_DECODER=OFF",
            "-DCHIAKI_ENABLE_PI_DECODER=OFF",
            "-DCHIAKI_ENABLE_SPEEX=OFF",
            "-DCHIAKI_LIB_ENABLE_OPUS=ON",
            "-DCHIAKI_ENABLE_RUDP=ON",
        ]);
        cfg.arg(format!(
            "-DCHIAKI_USE_SYSTEM_CURL={}",
            if system_curl_has_wss { "ON" } else { "OFF" }
        ));
        run_checked(&mut cfg, "cmake configure chiaki-lib");

        let jobs = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let mut build = Command::new("cmake");
        build.arg("--build").arg(build_dir).args([
            "--target",
            "chiaki-lib",
            "-j",
            &jobs.to_string(),
        ]);
        run_checked(&mut build, "cmake build chiaki-lib");
    }

    fn emit_chiaki_link_flags(build_dir: &Path, system_curl_has_wss: bool) {
        println!(
            "cargo:rustc-link-search=native={}",
            build_dir.join("lib").display()
        );
        println!(
            "cargo:rustc-link-search=native={}",
            build_dir.join("third-party").display()
        );
        println!(
            "cargo:rustc-link-search=native={}",
            build_dir.join("third-party").join("nanopb").display()
        );
        if !system_curl_has_wss {
            println!(
                "cargo:rustc-link-search=native={}",
                build_dir
                    .join("third-party")
                    .join("curl")
                    .join("lib")
                    .display()
            );
        }

        // Static libraries in dependency order (dependents before dependencies)
        println!("cargo:rustc-link-lib=static=chiaki");
        println!("cargo:rustc-link-lib=static=jerasure");
        println!("cargo:rustc-link-lib=static=gf_complete");
        println!("cargo:rustc-link-lib=static=protobuf-nanopb");
        if !system_curl_has_wss {
            println!("cargo:rustc-link-lib=static=curl");
        }

        // System shared libraries
        println!("cargo:rustc-link-lib=SDL2");
        println!("cargo:rustc-link-lib=opus");
        println!("cargo:rustc-link-lib=ssl");
        println!("cargo:rustc-link-lib=crypto");
        if system_curl_has_wss {
            println!("cargo:rustc-link-lib=curl");
        }
        println!("cargo:rustc-link-lib=event");
        println!("cargo:rustc-link-lib=json-c");
        println!("cargo:rustc-link-lib=miniupnpc");
        println!("cargo:rustc-link-lib=m");
        println!("cargo:rustc-link-lib=pthread");
    }
}
