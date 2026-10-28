#!/usr/bin/env python3
"""Exercise the real Deck C capture bridge, without Steam, a GPU, or host services.

Requires a C compiler, pkg-config, PipeWire development files/tools, and GStreamer
with videotestsrc and pipewiresink. Run: python3 scripts/test-steam-deck-capture.py
"""
import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", type=Path, help="Also exercise the portable PipeWire libraries and plugins")
    parser.add_argument("--probe", type=Path, help="Use a capture probe compiled on the target glibc baseline")
    parser.add_argument("--log-dir", type=Path, help="Keep complete subprocess logs for diagnostics")
    arguments = parser.parse_args()
    bundle = arguments.bundle
    bundle = bundle.resolve() if bundle else None
    with tempfile.TemporaryDirectory(prefix="michadame-pw-test-") as temporary:
        directory = Path(temporary)
        probe = arguments.probe.resolve() if arguments.probe else directory / "capture-probe"
        if not arguments.probe:
            flags = shlex.split(subprocess.check_output(
                ["pkg-config", "--cflags", "--libs", "libpipewire-0.3"], text=True))
            if bundle:
                flags = ["-L" + str(bundle / "lib"), "-Wl,-rpath," + str(bundle / "lib"), *flags]
            subprocess.run(["cc", "-Wall", "-Wextra", "-Werror",
                            str(ROOT / "tests/steam_deck_capture_probe.c"),
                            str(ROOT / "src/steam_deck/capture.c"), *flags,
                            "-o", str(probe)], check=True)
        config = directory / "pipewire.conf"
        config.write_text("""context.properties = { core.daemon = true core.name = pipewire-0 mem.warn-mlock = false }
context.spa-libs = { support.* = support/libspa-support }
context.modules = [
 { name = libpipewire-module-protocol-native }
 { name = libpipewire-module-access }
 { name = libpipewire-module-client-node }
 { name = libpipewire-module-spa-node-factory }
 { name = libpipewire-module-adapter }
 { name = libpipewire-module-link-factory }
 { name = libpipewire-module-metadata }
]
context.objects = [
 { factory = spa-node-factory args = { factory.name = support.node.driver node.name = Dummy-Driver node.group = pipewire.dummy priority.driver = 20000 } }
]
""")
        environment = {**os.environ, "XDG_RUNTIME_DIR": temporary,
                       "PIPEWIRE_RUNTIME_DIR": temporary, "PIPEWIRE_REMOTE": "pipewire-0"}
        client_environment = environment
        if bundle:
            client_environment = {**environment,
                "PIPEWIRE_CONFIG_DIR": str(bundle / "share/pipewire"),
                "PIPEWIRE_MODULE_DIR": str(bundle / "lib/pipewire-0.3"),
                "SPA_PLUGIN_DIR": str(bundle / "lib/spa-0.2")}
            client_environment.pop("LD_LIBRARY_PATH", None)
        processes = []
        logs = []
        success = False

        def start(name, arguments, env=None):
            log = (directory / (name + ".log")).open("w+")
            logs.append((name, log))
            process = subprocess.Popen(arguments, env=env or environment, stdout=log, stderr=subprocess.STDOUT)
            processes.append(process)
            return process

        def output(arguments):
            return subprocess.check_output(arguments, env=environment, text=True, timeout=3)

        try:
            server = start("pipewire", ["pipewire", "-c", str(config)])
            deadline = time.monotonic() + 5
            while not (directory / "pipewire-0").exists():
                if server.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError("Private PipeWire daemon did not start")
                time.sleep(0.03)
            publisher = start("publisher", ["gst-launch-1.0", "-q", "videotestsrc", "is-live=true",
                "pattern=red", "!", "video/x-raw,format=BGRx,width=1280,height=800,framerate=60/1",
                "!", "pipewiresink", "mode=render",
                "stream-properties=props,node.name=deck-fixture,media.class=Video/Source,node.want-driver=true"],
                {**environment, "PIPEWIRE_PROPS": "michadame.deck.session=deck-fixture-session"})
            deadline = time.monotonic() + 5
            while True:
                dump = subprocess.check_output([str(bundle / "bin/pw-dump") if bundle else "pw-dump"],
                    env=client_environment, text=True, timeout=3)
                if arguments.log_dir:
                    arguments.log_dir.mkdir(parents=True, exist_ok=True)
                    (arguments.log_dir / "pw-dump.json").write_text(dump)
                source = next((obj for obj in json.loads(dump)
                    if obj.get("info", {}).get("props", {}).get("node.name") == "deck-fixture"), None)
                if source:
                    assert source["info"]["props"]["michadame.deck.session"] == "deck-fixture-session"
                    break
                if publisher.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError("No fixture video source")
                time.sleep(0.03)
            capture = start("capture", [str(probe), str(source["info"]["props"]["object.serial"])], client_environment)
            # The private daemon deliberately has no session manager. Create the
            # link explicitly after both ports appear; host routing is untouched.
            deadline = time.monotonic() + 5
            while True:
                source_port = next((port.strip() for port in output(["pw-link", "-o"]).splitlines()
                    if port.strip().startswith("deck-fixture:")), None)
                capture_port = next((port.strip() for port in output(["pw-link", "-i"]).splitlines()
                    if port.strip().startswith("michadame-deck-video:")), None)
                if source_port and capture_port:
                    subprocess.run(["pw-link", source_port, capture_port], env=environment, check=True, timeout=3)
                    break
                if capture.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError("Video capture ports did not appear")
                time.sleep(0.03)
            if capture.wait(timeout=10) != 0:
                raise RuntimeError("Video frame validation failed")
            success = True
            print("Native PipeWire shared-memory integration passed: 30 red 1280x800 frames")
        finally:
            for process in reversed(processes):
                if process.poll() is None:
                    process.terminate()
            for process in reversed(processes):
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            for name, log in logs:
                if arguments.log_dir:
                    arguments.log_dir.mkdir(parents=True, exist_ok=True)
                    log.flush()
                    shutil.copy2(log.name, arguments.log_dir / (name + ".log"))
                if not success:
                    log.flush()
                    log.seek(0)
                    print(name + ":\n" + log.read()[-3500:])
                log.close()


if __name__ == "__main__":
    main()
