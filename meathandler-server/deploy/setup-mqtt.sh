#!/usr/bin/env bash
# Install a LAN-only MQTT broker for the WiCAN's battery alerts and point
# meathandler at it. Run on the Pi:  sudo ./setup-mqtt.sh
#
# Safe to re-run: it rotates both broker passwords and updates everything
# that uses them.
set -euo pipefail
cd "$(dirname "$0")"
HOST_IP="${HOST_IP:-$(hostname -I | awk '{print $1}')}"
ENV=/etc/meathandler/env
PASSWD=/etc/mosquitto/meathandler.passwd
TOPIC=meathandler/battery
USER_HOME="$(getent passwd "${SUDO_USER:-root}" | cut -d: -f6)"
NOTE="$USER_HOME/meathandler-install/wican-mqtt.txt"

[ -f "$ENV" ] || { echo "Run install.sh first"; exit 1; }

apt-get install -y -qq mosquitto mosquitto-clients >/dev/null

WICAN_PASS="$(openssl rand -hex 12)"
MH_PASS="$(openssl rand -hex 12)"
rm -f "$PASSWD"
mosquitto_passwd -b -c "$PASSWD" wican "$WICAN_PASS"
mosquitto_passwd -b "$PASSWD" meathandler "$MH_PASS"
chown root:mosquitto "$PASSWD"
chmod 640 "$PASSWD"

# Listen on the LAN address and localhost only. Port 1883 is not forwarded
# on the router, so this is never reachable from the internet.
cat > /etc/mosquitto/conf.d/meathandler.conf <<CONF
listener 1883 $HOST_IP
listener 1883 127.0.0.1
allow_anonymous false
password_file $PASSWD
CONF

systemctl enable mosquitto >/dev/null
systemctl restart mosquitto

set_env() {
  if grep -q "^$1=" "$ENV"; then sed -i "s|^$1=.*|$1=$2|" "$ENV"; else echo "$1=$2" >> "$ENV"; fi
}
set_env MH_MQTT_HOST 127.0.0.1
set_env MH_MQTT_PORT 1883
set_env MH_MQTT_USER meathandler
set_env MH_MQTT_PASS "$MH_PASS"
set_env MH_MQTT_TOPIC "$TOPIC"
systemctl restart meathandler

# The WiCAN's settings, for its Battery Alert form (or for Claude to apply).
cat > "$NOTE" <<NOTE
WiCAN Battery Alert settings (Settings > Sleep Mode > Battery Alert)
URL:      mqtt://$HOST_IP
Port:     1883
User:     wican
Password: $WICAN_PASS
Topic:    $TOPIC
NOTE
chown "${SUDO_USER:-root}" "$NOTE"
chmod 600 "$NOTE"

sleep 1
systemctl --no-pager --lines=0 status mosquitto | head -3
echo
echo "Broker ready. WiCAN settings saved to $NOTE"
