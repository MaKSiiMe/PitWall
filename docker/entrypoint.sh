#!/bin/sh
# PitWall runs as uid/gid 9999, the user of the Trackmania server image, so that it can write
# maps into the server's UserData volume. Started as root only to hand over /data (created by an
# older image as uid 10001), then drops privileges for good.
set -e
if [ "$(id -u)" = "0" ]; then
    if [ -d /data ] && [ "$(stat -c %u /data)" != "9999" ]; then
        chown -R 9999:9999 /data
    fi
    exec setpriv --reuid=9999 --regid=9999 --init-groups -- pitwall "$@"
fi
exec pitwall "$@"
