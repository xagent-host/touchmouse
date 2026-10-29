#!/usr/bin/env bash
# Install the prebuilt executable and controller independently of source.
set -euo pipefail

fail() { printf '%s\n' "$*" >&2; exit 1; }
[[ $EUID -eq 0 && $# -eq 1 ]] || fail "Usage: sudo bash install-system.sh DESKTOP_USER"
script_path=$(readlink -f -- "${BASH_SOURCE[0]}")
project_root=$(dirname -- "$script_path")
binary=$project_root/target/release/touchmouse
[[ -f $binary ]] || fail "Build first: cargo build --release --locked"
[[ -f $project_root/control ]] || fail "Missing control script: $project_root/control"
lib=/usr/local/lib/touchmouse
command_path=/usr/local/bin/touchmouse
# Refuse conflicting command entries before changing installed files or permissions.
if [[ -L $command_path && $(readlink -f -- "$command_path") == "$lib/control" ]]; then
    :
elif [[ -e $command_path || -L $command_path ]]; then
    fail "Refusing to overwrite existing $command_path"
fi
bash "$project_root/install-permissions.sh" "$1"
mkdir -p -- "$lib" /usr/local/bin
# Atomic replacement also permits upgrading an executable that is running.
for name in touchmouse control; do
    if [[ $name == touchmouse ]]; then source_path=$binary; else source_path=$project_root/control; fi
    install -o root -g root -m 0755 -- "$source_path" "$lib/$name.new"
    mv -fT -- "$lib/$name.new" "$lib/$name"
done
if [[ ! -L $command_path ]]; then
    ln -s -- "$lib/control" "$command_path"
fi
printf 'Installed to /usr/local/lib/touchmouse. Bind F9 to /usr/local/lib/touchmouse/control toggle.\n'
