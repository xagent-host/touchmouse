#!/usr/bin/env bash
# One-time administrator setup for persistent uinput access.
set -euo pipefail

fail() { printf '%s\n' "$*" >&2; exit 1; }
[[ $EUID -eq 0 && $# -eq 1 ]] || fail "Usage: sudo bash install-permissions.sh USER"
desktop_user=$1
[[ $desktop_user =~ ^[a-z_][a-z0-9_-]*$ ]] || fail "Invalid user name"
desktop_uid=$(id -u -- "$desktop_user") || fail "Unknown user: $desktop_user"
[[ $desktop_uid -ne 0 ]] || fail "Choose the desktop user, not root"

rule=/etc/udev/rules.d/99-touchmouse-uinput.rules
module=/etc/modules-load.d/touchmouse.conf
rule_content=$(printf '# Touchmouse: persistent virtual mouse access for the desktop user.\nSUBSYSTEM=="misc", KERNEL=="uinput", OWNER="%s", MODE="0600"' "$desktop_user")
# Validate both configurations before writing either one.
if [[ -e $rule || -L $rule ]]; then
    cmp -s "$rule" <(printf '%s\n' "$rule_content") || fail "Refusing to overwrite different existing rule: $rule"
fi
if [[ -e $module || -L $module ]]; then
    module_content=$(cat -- "$module")
    [[ $module_content =~ ^[[:space:]]*uinput[[:space:]]*$ ]] || fail "Refusing to overwrite different module configuration: $module"
fi
printf '%s\n' "$rule_content" > "$rule"
chmod 0644 "$rule"
printf 'uinput\n' > "$module"
chmod 0644 "$module"
modprobe uinput
udevadm control --reload-rules
udevadm trigger --action=change --subsystem-match=misc --sysname-match=uinput
udevadm settle
[[ $(stat -c '%u' /dev/uinput) == "$desktop_uid" ]] || fail "Rule installed but device ownership did not change; check later udev rules"
printf 'Persistent uinput access installed for %s; F9 no longer needs authentication after reboot.\n' "$desktop_user"
