#!/bin/sh
# Runs the whole site locally (or on the port in $PORT): the docs at
# http://localhost:8000 and the editor at http://editor.localhost:8000
# (browsers send every *.localhost name to this machine). It builds the
# docs and the editor, seeds a demo database on the first run (users ella,
# alice, bob, and carol, password "password123"), and starts the server in
# development mode, keeping ella's example sketches in step with the docs.
set -eu
cd "$(dirname "$0")"
PORT="${PORT:-8000}"
export EASL_EDITOR_ORIGIN="http://editor.localhost:$PORT"
(cd generator && cargo run)
cd server
[ -f data/easl-site.db ] || cargo run -q -- seed
exec cargo run -q -- serve --dev --bind "127.0.0.1:$PORT" --sync-examples-owner ella
