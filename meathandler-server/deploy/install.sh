#!/usr/bin/env bash
# Run on the Pi from the folder holding the binary and these files:
#   sudo ./install.sh
# Safe to re-run: keeps the existing CA, token and dashboard password.
set -euo pipefail
cd "$(dirname "$0")"
HOST_IP="${HOST_IP:-$(hostname -I | awk '{print $1}')}"
PUBLIC_HOST="${PUBLIC_HOST:-meathandler.ddns.net}"
USER_HOME="$(getent passwd "${SUDO_USER:-root}" | cut -d: -f6)"
SSL_DIR="${SSL_DIR:-$USER_HOME/meathandler-ssl}"
ENV=/etc/meathandler/env
CERTS=/etc/meathandler/certs

id meathandler >/dev/null 2>&1 || useradd --system --home /var/lib/meathandler --shell /usr/sbin/nologin meathandler
install -d -o meathandler -g meathandler -m 750 /var/lib/meathandler
install -d -m 755 /etc/meathandler
install -m 755 meathandler-server /usr/local/bin/meathandler-server

# The Slint dashboard for the browser (built by build-web.sh), if shipped.
if [ -f web/index.html ]; then
  rm -rf /usr/local/share/meathandler/web
  install -d -m 755 /usr/local/share/meathandler/web
  install -m 644 web/* /usr/local/share/meathandler/web/
  echo "Installed web dashboard"
fi

# Local CA cert, for clients that connect by IP.
if [ ! -f "$CERTS/server.crt" ]; then
  bash ./gen-certs.sh "$HOST_IP" "$CERTS"
fi

# Public No-IP cert, if one has been downloaded next to its key.
PUB_CERT="$(ls "$SSL_DIR"/*.pem 2>/dev/null | head -1 || true)"
if [ -n "$PUB_CERT" ] && [ -f "$SSL_DIR/$PUBLIC_HOST.key" ]; then
  install -m 644 "$PUB_CERT" "$CERTS/public.crt"
  install -m 640 "$SSL_DIR/$PUBLIC_HOST.key" "$CERTS/public.key"
  echo "Installed public certificate from $PUB_CERT"
fi
chown -R root:meathandler "$CERTS"
chmod 640 "$CERTS/server.key"
chmod 600 "$CERTS/ca.key"

touch "$ENV"
set_default() { grep -q "^$1=" "$ENV" || echo "$1=$2" >> "$ENV"; }
set_default MH_TOKEN "$(openssl rand -hex 24)"
set_default MH_DASH_USER enoch
set_default MH_DASH_PASS "$(openssl rand -base64 18 | tr -d '/+=' | cut -c1-16)"
set_default MH_DB /var/lib/meathandler/meathandler.db
set_default MH_TLS_CERT "$CERTS/server.crt"
set_default MH_TLS_KEY "$CERTS/server.key"
if [ -f "$CERTS/public.crt" ]; then
  set_default MH_PUBLIC_HOST "$PUBLIC_HOST"
  set_default MH_PUBLIC_CERT "$CERTS/public.crt"
  set_default MH_PUBLIC_KEY "$CERTS/public.key"
fi
set_default MH_WEB_DIR /usr/local/share/meathandler/web
set_default MH_HTTP_ADDR 0.0.0.0:8080
set_default MH_HTTPS_ADDR 0.0.0.0:8443
set_default RUST_LOG info
# Phone alerts via ntfy. The topic name is the secret, so make it unguessable.
set_default MH_NTFY_TOPIC "meathandler-$(openssl rand -hex 12)"
set_default MH_PLUG_ALERT_SECS 120
set_default MH_NOTIFY_CHARGE_START 1
chown root:meathandler "$ENV"
chmod 640 "$ENV"

install -m 644 meathandler.service /etc/systemd/system/meathandler.service
systemctl daemon-reload
systemctl enable meathandler >/dev/null
systemctl restart meathandler
sleep 1
systemctl --no-pager --lines=5 status meathandler || true

echo
echo "Dashboard:  https://$PUBLIC_HOST:8443/  (or http://$HOST_IP:8080/ at home)"
echo "Login:      $(grep ^MH_DASH_USER= "$ENV" | cut -d= -f2) / $(grep ^MH_DASH_PASS= "$ENV" | cut -d= -f2)"
echo "WiCAN token: $(grep ^MH_TOKEN= "$ENV" | cut -d= -f2)"
TOPIC="$(grep ^MH_NTFY_TOPIC= "$ENV" | cut -d= -f2)"
echo "ntfy topic: $TOPIC   (subscribe to this in the ntfy app)"
if [ -n "${SEND_TEST:-1}" ] && [ "${SEND_TEST:-1}" != 0 ]; then
  curl -s -m 10 -H "Title: MeatHandler alerts are set up" -d "You will get a ping here if the EV6 is plugged in but not charging." \
    "https://ntfy.sh/$TOPIC" >/dev/null && echo "Sent a test notification to the topic."
fi
