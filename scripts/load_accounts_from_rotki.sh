#! /bin/sh
set -e

if [ -f .env ]; then
  set -o allexport
  . ./.env
  set +o allexport
fi

curl http://localhost:4242/api/1/blockchains/eth/accounts | \
  jq -c '.result[] | {address: .address, label: .label}' | \
  tr '\n' '\0' | \
  xargs -0 -I % \
  curl --request POST \
    --url $LOADING_SCRIPTS_HOST/accounts \
    --header 'Content-Type: application/json' \
    --data %
