#!/bin/sh
set -eu

package_path=${1:?usage: smoke-test.sh PACKAGE}
test -f "$package_path"

case "$package_path" in
    *.deb)
        apt-get update
        DEBIAN_FRONTEND=noninteractive apt-get install --yes systemd passwd
        dpkg --install "$package_path"
        ;;
    *.rpm)
        dnf install --assumeyes systemd shadow-utils
        rpm --install --replacepkgs "$package_path"
        ;;
    *) printf 'unsupported package: %s\n' "$package_path" >&2; exit 2 ;;
esac

test -x /usr/bin/syspilot
test -f /usr/lib/systemd/system/syspilot.service
id syspilot >/dev/null
test "$(stat -c '%U:%G:%a' /var/lib/syspilot)" = "syspilot:syspilot:750"
test "$(stat -c '%U:%G:%a' /etc/syspilot)" = "root:syspilot:750"

printf 'preserved across package upgrades\n' >/etc/syspilot/upgrade-marker
case "$package_path" in
    *.deb) dpkg --install "$package_path" ;;
    *.rpm) rpm --upgrade --replacepkgs "$package_path" ;;
esac
test "$(cat /etc/syspilot/upgrade-marker)" = "preserved across package upgrades"
/usr/bin/syspilot --version
