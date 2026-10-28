#!/usr/bin/env bash
# Writes only a per-user desktop shortcut; Steam's shortcut database is untouched.
set -euo pipefail
bundle_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
applications_dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
mkdir -p "$applications_dir"
python3 - "$bundle_dir" "$applications_dir/michadame-steam-deck.desktop" <<'PY'
from pathlib import Path
import sys
root, target = sys.argv[1:]
def quoted(value):
    return '"' + value.replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$').replace('%', '%%') + '"'
Path(target).write_text('[Desktop Entry]\nType=Application\nName=Michadame Steam Deck\n'
    + 'Exec=' + quoted(root + '/michadame-steam-deck') + '\n'
    + 'Icon=' + root + '/michadame.png\nTerminal=false\nCategories=Game;\n')
print('Created ' + target)
PY
printf 'In Steam: Games → Add a Non-Steam Game → Michadame Steam Deck.\n'
