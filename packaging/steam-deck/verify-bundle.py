#!/usr/bin/env python3
"""Run in the minimal runtime-test image against the extracted portable folder."""
import ctypes
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

bundle = Path(sys.argv[1]).resolve()
manifest = json.loads((bundle / "share/michadame/bundle-manifest.json").read_text())
for name, expected in manifest["files"].items():
    assert hashlib.sha256((bundle / name).read_bytes()).hexdigest() == expected, name
objects = [path for path in bundle.rglob("*") if path.is_file() and path.read_bytes()[:4] == b"\x7fELF"]
highest = (0, 0)
for path in objects:
    resolved = subprocess.check_output(["ldd", str(path)], text=True)
    assert "not found" not in resolved, resolved
    versions = subprocess.check_output(["readelf", "--version-info", str(path)], text=True)
    required = [tuple(map(int, match.split("."))) for match in re.findall(r"Name: GLIBC_([0-9.]+)", versions)]
    highest = max([highest, *required])
assert highest <= (2, 35), highest
print(f"Verified {len(objects)} ELF objects; maximum required glibc: {'.'.join(map(str, highest))}", flush=True)

for name in ["libei.so.1", "libSDL2-2.0.so.0", "libasound.so.2", "alsa-lib/libasound_module_pcm_pulse.so"]:
    ctypes.CDLL(str(bundle / "lib" / name))
print("Input and ALSA PulseAudio plugins load", flush=True)
encoders = subprocess.check_output([str(bundle / "bin/ffmpeg"), "-hide_banner", "-encoders"], text=True, stderr=subprocess.DEVNULL)
assert "h264_vaapi" in encoders and "aac" in encoders
subprocess.run([str(bundle / "bin/ffmpeg"), "-v", "error", "-f", "lavfi", "-i",
                "color=red:size=1280x800:rate=60", "-frames:v", "3", "-f", "null", "-"], check=True)
print("FFmpeg codecs and generated video pass", flush=True)

with tempfile.TemporaryDirectory(prefix="michadame-portable-test-") as temporary:
    environment = {**os.environ, "XDG_CONFIG_HOME": temporary + "/config",
                   "XDG_DATA_HOME": temporary + "/data", "XDG_CACHE_HOME": temporary + "/cache",
                   "XDG_RUNTIME_DIR": temporary, "XDG_STATE_HOME": temporary + "/state", "LIBGL_ALWAYS_SOFTWARE": "1", "RUST_LOG": "info"}
    for key in ["LD_LIBRARY_PATH", "PIPEWIRE_MODULE_DIR", "PIPEWIRE_CONFIG_DIR", "SPA_PLUGIN_DIR", "ALSA_CONFIG_PATH"]:
        environment.pop(key, None)
    subprocess.run([str(bundle / "install-steam-shortcut.sh")], env=environment, check=True)
    desktop = Path(temporary + "/data/applications/michadame-steam-deck.desktop").read_text()
    assert str(bundle) in desktop and "Terminal=false" in desktop
    result = subprocess.run(["xvfb-run", "-a", "-s", "-screen 0 1280x800x24", "timeout", "15s",
                             str(bundle / "michadame-steam-deck")], env=environment,
                            text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    print(result.stdout[-5000:], flush=True)
    assert result.returncode == 124, f"Application exited early: {result.returncode}"
    assert "panicked at" not in result.stdout and "Application error" not in result.stdout
    assert "Cannot access file" not in result.stdout and "Unknown PCM" not in result.stdout
print("Fullscreen application survives 15 seconds in the minimal runtime with software OpenGL", flush=True)
