#!/bin/sh
# Let's Encrypt certificate for prswt.juz.ai via DNS-01 (Cloudflare DNS token on stdin).
# Files land in ~/prswt/deploy/tls/certificates/, owned by srs. Usage: le_cert.sh run|renew
set -e
read -r T
mkdir -p "$HOME/prswt/deploy/tls"
chmod 700 "$HOME/prswt/deploy/tls"
if [ "$1" = renew ]; then ACTION="renew --days 30"; else ACTION="run"; fi
docker run --rm --user "$(id -u):$(id -g)" -e CF_DNS_API_TOKEN="$T" -e LEGO_DISABLE_CNAME_SUPPORT=true \
  -v "$HOME/prswt/deploy/tls:/data" goacme/lego:v4.26.0 \
  --path /data --accept-tos --email bot@juz.ai --dns cloudflare --dns.resolvers 1.1.1.1:53 \
  --domains prswt.juz.ai $ACTION 2>&1 | tail -4
ls -l "$HOME/prswt/deploy/tls/certificates/" 2>/dev/null | grep prswt
openssl x509 -in "$HOME/prswt/deploy/tls/certificates/prswt.juz.ai.crt" -noout -subject -issuer -enddate 2>/dev/null || true
