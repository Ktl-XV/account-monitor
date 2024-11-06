#! /bin/sh
set -e

if [ -f .env ]; then
  set -o allexport
  . ./.env
  set +o allexport
fi


yq -o=json -I=0 '.[]' $1 | \
  tr '\n' '\0' | \
  xargs -0 -I % \
  curl --request POST \
    --url $LOADING_SCRIPTS_HOST/accounts \
    --header 'Content-Type: application/json' \
    --data %

