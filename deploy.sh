#!/bin/sh
# Builds the site from scratch and uploads it to easl-lang.org's server,
# replacing what's there. Uploads go in as the server's `deploy` user,
# through the `easl-site` SSH host (see README.md, "Deploying").
set -eu
cd "$(dirname "$0")"
rm -rf site
(cd generator && cargo run)
rsync -az --delete site/ deploy@easl-site:/srv/easl-site/
echo "deployed to https://easl-lang.org"
