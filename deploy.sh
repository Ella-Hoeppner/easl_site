#!/bin/sh
# Deploys easl-lang.org through the `easl-site` SSH host (see
# server/README.md, "Production"):
#
#   ./deploy.sh          the docs site, then the editor server
#   ./deploy.sh site     builds the docs site from scratch and uploads it
#                        (as the server's `deploy` user), replacing what's
#                        there
#   ./deploy.sh server   uploads the server's source, builds it on the
#                        machine, installs the binary, and restarts the
#                        service if it's enabled
#
# The site goes first: the server syncs the examples from it when it starts.
set -eu
cd "$(dirname "$0")"
what="${1:-all}"
case "$what" in
  all | site | server) ;;
  *) echo "usage: $0 [site|server]" >&2; exit 2 ;;
esac

if [ "$what" != server ]; then
  # Deploys build the pinned easl releases, never local checkouts.
  if [ -f .cargo/config.toml ]; then
    echo "the local easl override is on (.cargo/config.toml); delete it to deploy" >&2
    exit 1
  fi
  rm -rf site
  # The production editor origin: dev.sh's override must not leak in.
  (cd generator && env -u EASL_EDITOR_ORIGIN -u EASL_DIR -u EASL_LSP_DIR cargo run)
  rsync -az --delete site/ deploy@easl-site:/srv/easl-site/
  echo "deployed the docs to https://easl-lang.org"
fi

if [ "$what" != site ]; then
  rsync -az --delete --exclude target --exclude data server/ \
    deploy@easl-site:easl-site-server/
  ssh deploy@easl-site \
    'cd easl-site-server && ~/.cargo/bin/cargo build --release --locked --quiet'
  ssh easl-site '
    set -e
    install -m 755 /home/deploy/easl-site-server/target/release/easl-site-server /usr/local/bin/
    if systemctl is-enabled --quiet easl-site; then
      systemctl restart easl-site
      sleep 1
      systemctl is-active --quiet easl-site || {
        journalctl -u easl-site -n 30 --no-pager
        exit 1
      }
      echo "restarted the editor at https://editor.easl-lang.org"
    else
      echo "installed the server binary (the easl-site service is not enabled)"
    fi'
fi
