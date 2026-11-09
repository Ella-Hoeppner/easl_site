#!/bin/sh
# Runs the whole site locally, at http://127.0.0.1:8000 (or the port in
# $PORT): builds the docs and the editor, seeds a demo database on the
# first run (users alice, bob, and carol, password "password123"), and
# starts the server in development mode.
set -eu
cd "$(dirname "$0")"
PORT="${PORT:-8000}"
(cd generator && cargo run)
cd server
[ -f data/easl-site.db ] || cargo run -q -- seed
exec cargo run -q -- serve --dev --bind "127.0.0.1:$PORT"
