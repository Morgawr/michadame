#!/usr/bin/env python3
"""Assemble the portable folder inside the Ubuntu 22.04 build container."""
import fnmatch
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
DEST = Path(sys.argv[1]).resolve()
HOST = [
    "ld-linux*", "libc.so.*", "libm.so.*", "libpthread.so.*", "libdl.so.*",
    "librt.so.*", "libresolv.so.*", "libutil.so.*", "libgcc_s.so.*", "libstdc++.so.*",
    "libGL*.so.*", "libEGL.so.*", "libGLES*.so.*", "libvulkan.so.*", "libgbm.so.*",
    "libdrm*.so.*", "libva*.so.*", "libudev.so.*",
    "libX11.so.*", "libX11-xcb.so.*", "libxcb*.so.*", "libwayland*.so.*",
    "libxkbcommon*.so.*",
]
libraries = {}
excluded = set()
objects = []


def run(*args):
    return subprocess.check_output(args, text=True)


def dependencies(source):
    needed = re.findall(r"\(NEEDED\).*\[(.*?)\]", run("readelf", "-d", str(source)))
    if not needed:
        return
    resolved = dict(re.findall(r"^\s*(\S+) => (/\S+)", run("ldd", str(source)), re.M))
    for name in needed:
        if any(fnmatch.fnmatchcase(name, pattern) for pattern in HOST):
            excluded.add(name)
            continue
        if name in libraries:
            continue
        if name not in resolved:
            raise RuntimeError(f"Unresolved library {name} needed by {source}")
        original = Path(resolved[name])
        libraries[name] = str(original.resolve())
        copy_elf(original, DEST / "lib" / name)


def copy_elf(source, target):
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, target, follow_symlinks=True)
    objects.append(target)
    dependencies(source)


if DEST.exists():
    shutil.rmtree(DEST)
DEST.mkdir(parents=True)
copy_elf(Path(os.environ["CARGO_TARGET_DIR"]) / "release/michadame", DEST / "bin/michadame")
copy_elf(Path(shutil.which("pactl")), DEST / "bin/pactl")
copy_elf(Path(shutil.which("flock")), DEST / "bin/flock")
copy_elf(Path("/opt/michadame-deps/bin/pw-dump"), DEST / "bin/pw-dump")
copy_elf(Path("/opt/michadame-deps/bin/ffmpeg"), DEST / "bin/ffmpeg")
copy_elf(Path("/opt/michadame-deps/bin/ffprobe"), DEST / "bin/ffprobe")

# These libraries/plugins are loaded with dlopen and do not appear in DT_NEEDED.
base = Path("/usr/lib/x86_64-linux-gnu")
for source in [base / "libSDL2-2.0.so.0", base / "alsa-lib/libasound_module_pcm_pulse.so",
               base / "alsa-lib/libasound_module_ctl_pulse.so"]:
    relative = source.relative_to(base)
    copy_elf(source, DEST / "lib" / relative)
pwbase = Path("/opt/michadame-deps/lib")
for module in ["rt", "protocol-native", "client-node", "client-device", "adapter", "metadata", "session-manager"]:
    source = pwbase / "pipewire-0.3" / f"libpipewire-module-{module}.so"
    if source.is_file():
        copy_elf(source, DEST / "lib/pipewire-0.3" / source.name)
for folder in ["support", "audioconvert", "audiomixer", "videoconvert"]:
    for source in (pwbase / "spa-0.2" / folder).glob("*.so"):
        copy_elf(source, DEST / "lib/spa-0.2" / folder / source.name)

shutil.copytree("/opt/michadame-deps/share/pipewire", DEST / "share/pipewire")
shutil.copytree("/usr/share/alsa", DEST / "share/alsa", symlinks=False)
# Do not mix the host's ALSA plugin snippets with this private libasound build.
alsa = DEST / "share/alsa/alsa.conf"
text = alsa.read_text()
hooks_start = text.index("@hooks [")
hooks_end = text.index("\n]", hooks_start) + 2
text = text[:hooks_start] + "# Host ALSA configuration is not loaded by this private bundle.\n" + text[hooks_end:]
pulse = Path("/usr/share/alsa/alsa.conf.d/50-pulseaudio.conf")
if pulse.exists():
    text += "\n" + pulse.read_text()
text += '\npcm.!default { type pulse }\nctl.!default { type pulse }\n'
alsa.write_text(text)

for path in objects:
    relative = os.path.relpath(DEST / "lib", path.parent)
    rpath = "$ORIGIN" if relative == "." else "$ORIGIN/" + relative
    # DT_RPATH also resolves dependencies of downloaded OCR/Libretro modules.
    subprocess.run(["patchelf", "--force-rpath", "--set-rpath", rpath, str(path)], check=True)
    subprocess.run(["strip", "--strip-unneeded", str(path)], check=True)

for name in ["michadame-steam-deck", "install-steam-shortcut.sh", "README.txt"]:
    shutil.copy2(ROOT / "packaging/steam-deck" / name, DEST / name)
for name in ["michadame-steam-deck", "install-steam-shortcut.sh"]:
    (DEST / name).chmod(0o755)
shutil.copy2(ROOT / "assets/logo.png", DEST / "michadame.png")
share = DEST / "share/michadame"
share.mkdir(parents=True)
shutil.copy2(ROOT / "LICENSE", share / "LICENSE")
licenses = DEST / "share/licenses"
licenses.mkdir(parents=True)
shutil.copytree("/opt/michadame-deps/share/licenses", licenses, dirs_exist_ok=True)
for copyright_file in Path("/usr/share/doc").glob("*/copyright"):
    # Keep distribution copyright notices with bundled third-party libraries.
    shutil.copy2(copyright_file, licenses / (copyright_file.parent.name + ".copyright"))
manifest = {
    "baseline": "Ubuntu 22.04, glibc 2.35, x86_64",
    "ffmpeg": "9.0.2", "libei": "1.5.0", "pipewire": "1.6.9",
    "sources": {
        "ffmpeg": {"url": "https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz",
                   "sha256": "8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e"},
        "libei": {"url": "https://deb.debian.org/debian/pool/main/libe/libei/libei_1.5.0.orig.tar.bz2",
                  "sha256": "da1fba92daccd0667bc46c3ee952d4ae8cfc6bdb4c0bb4d34df26528fb240618"},
        "pipewire": {"url": "https://deb.debian.org/debian/pool/main/p/pipewire/pipewire_1.6.9.orig.tar.gz",
                     "sha256": "a0012db8aaca4448fd205035e07d264235e4a4fe50713085fc4a6899e738a298"},
    },
    "libraries": libraries, "host_libraries": sorted(excluded),
    "source_revision": run("git", "-C", str(ROOT), "rev-parse", "HEAD").strip(),
    "source_has_local_changes": bool(run("git", "-C", str(ROOT), "status", "--porcelain").strip()),
    "files": {str(p.relative_to(DEST)): hashlib.sha256(p.read_bytes()).hexdigest()
              for p in sorted(DEST.rglob("*")) if p.is_file()},
}
(share / "bundle-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
print(f"Assembled {DEST}: {len(libraries)} bundled libraries, {len(objects)} ELF objects")
