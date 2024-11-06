#! /bin/sh

cp rotki-assets/databases/v9_global.db rotki_db.db

for update in $(jq '.updates | to_entries[] | select(.value.max_schema_version == 9) | .key | tonumber' rotki-assets/updates/info.json); do
  sed "s/\"/'/g" rotki-assets/updates/$update/* | grep -v "^*$" | sqlite3 rotki_db.db
done

sqlite3 rotki_db.db "SELECT 'DROP TABLE ' || name || ';' FROM sqlite_master WHERE type = 'table' AND name NOT IN ('evm_tokens', 'common_asset_details');" | xargs -i sqlite3 rotki_db.db "{}"
