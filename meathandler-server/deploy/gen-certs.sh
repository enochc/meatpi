#!/usr/bin/env bash
# Make a private CA and a server cert for the MeatHandler.
# Upload ca.crt to the WiCAN as a custom cert set so it trusts the server.
# Usage: gen-certs.sh <server-ip> <out-dir>
set -euo pipefail
HOST_IP="${1:?server IP, e.g. 10.43.1.57}"
OUT="${2:-certs}"
mkdir -p "$OUT"
cd "$OUT"

# The CA is kept across runs so the dongle does not need a new upload
# when the server cert is reissued (e.g. after the Pi's IP changes).
if [ ! -f ca.key ]; then
  openssl ecparam -name prime256v1 -genkey -noout -out ca.key
  openssl req -x509 -new -key ca.key -sha256 -days 3650 \
    -subj "/CN=MeatHandler Local CA" \
    -addext "basicConstraints=critical,CA:TRUE" \
    -addext "keyUsage=critical,keyCertSign,cRLSign" \
    -out ca.crt
fi

openssl ecparam -name prime256v1 -genkey -noout -out server.key
openssl req -new -key server.key -subj "/CN=$HOST_IP" -out server.csr
# The IP is listed as both an IP and a DNS name: some embedded TLS stacks
# only compare the URL host against DNS entries.
cat > server.ext <<EXT
basicConstraints=CA:FALSE
keyUsage=critical,digitalSignature,keyAgreement
extendedKeyUsage=serverAuth
subjectAltName=IP:$HOST_IP,DNS:$HOST_IP,DNS:meathandler.local,DNS:MeatHandler.local
EXT
openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial \
  -days 825 -sha256 -extfile server.ext -out server.crt
rm -f server.csr server.ext
echo "Wrote $(pwd)/server.crt for $HOST_IP (CA: $(pwd)/ca.crt)"
