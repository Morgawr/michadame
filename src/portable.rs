//! Runtime paths for the optional portable Linux distribution. A normal desktop
//! build has no bundle manifest and keeps its existing environment and tools.
use std::ffi::OsString;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

struct Runtime {
    root: PathBuf,
    original_environment: Vec<(&'static str, Option<OsString>)>,
    // ALSA_CONFIG_PATH splits on whitespace, even inside quotes. An open
    // directory gives ALSA a stable path without spaces for this process only.
    _bundle_directory: File,
}

static RUNTIME: OnceLock<Option<Runtime>> = OnceLock::new();

fn environment(root: &Path, alsa_root: &Path) -> Vec<(&'static str, PathBuf)> {
    vec![
        ("ALSA_CONFIG_PATH", alsa_root.join("share/alsa/alsa.conf")),
        ("ALSA_CONFIG_DIR", alsa_root.join("share/alsa")),
        ("ALSA_PLUGIN_DIR", alsa_root.join("lib/alsa-lib")),
        ("PIPEWIRE_CONFIG_DIR", root.join("share/pipewire")),
        ("PIPEWIRE_MODULE_DIR", root.join("lib/pipewire-0.3")),
        ("SPA_PLUGIN_DIR", root.join("lib/spa-0.2")),
    ]
}

/// Call before starting any threads or opening audio/video devices.
pub fn initialize() {
    RUNTIME.get_or_init(|| {
        let root = std::env::current_exe()
            .ok()?
            .parent()?
            .parent()?
            .to_path_buf();
        if !root.join("share/michadame/bundle-manifest.json").is_file() {
            return None;
        }
        let bundle_directory = File::open(&root).ok()?;
        let alsa_root = PathBuf::from(format!("/proc/self/fd/{}", bundle_directory.as_raw_fd()));
        let mut original_environment = Vec::new();
        for (key, path) in environment(&root, &alsa_root) {
            original_environment.push((key, std::env::var_os(key)));
            std::env::set_var(key, path);
        }
        Some(Runtime {
            root,
            original_environment,
            _bundle_directory: bundle_directory,
        })
    });
}

pub fn helper_path(name: &str) -> PathBuf {
    RUNTIME
        .get()
        .and_then(Option::as_ref)
        .map(|runtime| runtime.root.join("bin").join(name))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

pub fn helper_command(name: &str) -> Command {
    Command::new(helper_path(name))
}

/// Steam and its games must use the host's PipeWire/ALSA plugins. Libraries are
/// located with ELF rpaths, so no LD_LIBRARY_PATH is exported to child processes.
pub fn host_command(name: &str) -> Command {
    let mut command = Command::new(name);
    if let Some(Some(runtime)) = RUNTIME.get() {
        restore_host_environment(&mut command, &runtime.original_environment);
    }
    command
}

fn restore_host_environment(
    command: &mut Command,
    original_environment: &[(&str, Option<OsString>)],
) {
    for (key, original) in original_environment {
        match original {
            Some(value) => {
                command.env(key, value);
            }
            None => {
                command.env_remove(key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_child_receives_original_plugins_instead_of_bundle_plugins() {
        let mut command = Command::new("sh");
        command
            .env("PIPEWIRE_CONFIG_DIR", "/bundle/pipewire")
            .env("ALSA_PLUGIN_DIR", "/bundle/alsa-lib")
            .env("MICHA_TEST_PRESERVE", "keep me");
        restore_host_environment(
            &mut command,
            &[
                (
                    "PIPEWIRE_CONFIG_DIR",
                    Some("/original path with spaces".into()),
                ),
                ("ALSA_PLUGIN_DIR", None),
            ],
        );
        let output = command.args(["-c", "printf '%s|%s|%s' \"$PIPEWIRE_CONFIG_DIR\" \"${ALSA_PLUGIN_DIR-unset}\" \"$MICHA_TEST_PRESERVE\""])
            .output().unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "/original path with spaces|unset|keep me"
        );
    }
}
