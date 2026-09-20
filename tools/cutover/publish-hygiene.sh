#!/bin/sh

set -eu

repo_root=$(CDPATH='' cd "$(dirname "$0")/../.." && pwd -P)
hub_root=${ALPHAZEDEHQ_ROOT:-$(CDPATH='' cd "$repo_root/.." && pwd -P)}

exec node "$hub_root/tools/publish-hygiene/guard.mjs" \
  --repo "$repo_root" \
  --export-manifest public-export.json
