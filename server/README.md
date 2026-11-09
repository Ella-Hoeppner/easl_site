# easl site server

The dynamic half of easl-lang.org: accounts, sketches (multi-file easl programs people save and share), likes, comments, playlists, and moderation, as server-rendered pages and a JSON API. It also serves the generated static docs site (`../site`, built by `../generator`), so in production it's the only thing behind Caddy.

One Rust binary (axum + SQLite via sqlx + minijinja templates), built for a small VM (1 vCPU, 1 GB RAM).

## Running it in development

```sh
cd ../generator && cargo run        # build the static site into ../site
cd ../server
cargo run -- seed                   # demo users alice/bob/carol (password: password123), sketches, a playlist
cargo run -- serve --dev            # http://127.0.0.1:8000
```

`--dev`:

- accepts `http://localhost:<port>` and `http://127.0.0.1:<port>` as request origins (besides `--origin`),
- sets cookies without `Secure`, so sign-in works over plain http,
- reloads templates (`templates/`) and app assets (`static/`) from disk on every request, so design changes show on refresh,
- logs emails instead of sending them (unless SMTP is configured) and shows verification and reset links on the page in a "Dev mode" box.

`cargo run -- admin grant alice` makes a user an admin; it takes effect on their next request, and `/admin` appears in their account menu.

Tests: `cargo test` (integration tests in `tests/` run the real router against a fresh SQLite file each). `cargo clippy --all-targets` and `cargo fmt` should be clean.

## Commands and configuration

```
easl-site-server serve [--bind 127.0.0.1:8000] [--site-dir ../site] [--db ./data/easl-site.db]
                       [--origin http://<bind>] [--dev] [--trust-proxy]
easl-site-server seed  [--db …] [--site-dir …]
easl-site-server admin grant|revoke <username> [--db …]
```

- `--origin` is the public origin (`https://easl-lang.org`). It's used for absolute links (emails, link previews) and the CSRF check, so it must match what browsers see. It defaults to `http://<bind>`.
- `--trust-proxy` takes the client address (for rate limiting) from the last hop of `X-Forwarded-For`, which Caddy appends. Only set it behind a proxy, or clients can pick their own address.
- `SMTP_URL` (e.g. `smtps://user:pass@smtp.example.com:465`) and `MAIL_FROM` (e.g. `easl <noreply@easl-lang.org>`) enable email. Without them, emails are logged, with a warning at startup outside dev.
- `RUST_LOG` sets log levels (default `info,tower_http=info,sqlx=warn`).
- The database is created and migrated (`migrations/`) on startup. Expired sessions and email tokens are deleted hourly.

The editor pages read the playground build's versions from `<site-dir>/playground/manifest.json` (`{"version", "monaco_version"}`), re-reading it when the file changes, so a site rebuild needs no restart. Templates and app assets are compiled into the binary in release builds, so deploying the server means deploying one file.

## How it's organized

| | |
|---|---|
| `src/main.rs` | the CLI |
| `src/app.rs` | the router and the layers every request passes through |
| `src/config.rs`, `src/state.rs` | configuration, rate limits, shared state |
| `src/db.rs`, `migrations/` | the pool (WAL, foreign keys, 4 connections), ids, the schema |
| `src/auth.rs` | passwords (argon2id), sessions, email tokens, the session middleware and `MaybeUser`/`ApiUser`/`PageUser` extractors |
| `src/security.rs` | the CSRF check and client addresses |
| `src/ratelimit.rs` | in-memory sliding-window limits |
| `src/users.rs`, `sketches.rs`, `playlists.rs`, `comments.rs`, `reports.rs` | the data layer and the API's JSON shapes |
| `src/visibility.rs` | who can see what (one rule, used everywhere) |
| `src/api/` | `/api/…` handlers |
| `src/pages/` | page handlers (`editor.rs` renders `editor.html` for `/playground`, `/new`, `/s/<id>`) |
| `src/templates.rs`, `templates/` | minijinja (autoescaped); `base.html` is the layout, `macros.html` the sketch card |
| `src/assets.rs`, `static/` | the pages' own CSS/JS at `/static/…?v=<hash>` |
| `src/site.rs` | serving the static site |
| `src/avatar.rs` | generated SVG avatars and placeholder thumbnails |
| `src/text.rs` | plain text → HTML (escape, line breaks, links), dates |
| `src/mail.rs` | SMTP / log / test mailers |
| `src/validate.rs` | every input rule |
| `src/seed.rs` | demo data |

### The editor template's context

`editor.html` gets `page_json` (already safe inside `<script type="application/json">`), `asset_version`, `monaco_version`, `page_title`, `og` (`title`, `description`, `image`, `url`), plus what every template gets: `me`, `path`, `login_url`, `dev`, and the `static(path)` function. `page_json` is `{"mode": "playground" | "new" | "sketch", "user": Me | null, "sketch": Sketch | null, "example": string | null, "playlist": {id, title, index, count, prev, next} | null, "csrf_ok": true}`.

### Routes

Pages: `/explore`, `/u/<username>` (`?tab=sketches|playlists|likes&page=`), `/u/<username>/avatar.svg`, `/p/<id>`, `/s/<id>` (`?playlist=`), `/s/<id>/thumb`, `/playground` (`?example=`), `/new`, `/login`, `/signup`, `POST /logout`, `/forgot`, `/reset/<token>`, `/verify/<token>`, `/settings` (forms post to `/settings/profile|email|password|verification|delete`), `/admin`. `/playground.html` redirects to `/playground`. Everything else is the static site.

API (`/api`): `GET me`, `GET me/sketches`, `GET me/playlists`; `POST sketches`; `GET|PUT|DELETE sketches/<id>`; `POST sketches/<id>/fork`; `PUT sketches/<id>/thumbnail`; `POST|DELETE sketches/<id>/like`; `GET|POST sketches/<id>/comments`; `DELETE comments/<id>`; `POST playlists`; `GET|PUT|DELETE playlists/<id>`; `POST playlists/<id>/items`; `DELETE playlists/<id>/items/<sketch_id>`; `PUT playlists/<id>/order`; `POST reports`; `POST admin/sketches/<id>/hide|unhide`, `admin/users/<username>/ban|unban`, `admin/reports/<id>/resolve`. Errors are `{"error": "…"}` with 400/401/403/404/409/413/429.

## Production

Build on the server (or a matching Linux machine): `cargo build --release`, then install `target/release/easl-site-server` as `/usr/local/bin/easl-site-server`. The site goes where `deploy.sh` already puts it (`/srv/easl-site`); the database lives outside it, in `/var/lib/easl-site/` (owned by a dedicated `easl` user). Memory use is small (tens of MB, plus ~19 MB per concurrent password hash, at most two at once).

`/etc/systemd/system/easl-site.service`:

```ini
[Unit]
Description=easl site server
After=network.target

[Service]
User=easl
Group=easl
ExecStart=/usr/local/bin/easl-site-server serve \
  --bind 127.0.0.1:8000 \
  --site-dir /srv/easl-site \
  --db /var/lib/easl-site/easl-site.db \
  --origin https://easl-lang.org \
  --trust-proxy
EnvironmentFile=/etc/easl-site.env
Restart=on-failure
StateDirectory=easl-site
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/easl-site
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

`/etc/easl-site.env` (mode 600) holds `SMTP_URL=…` and `MAIL_FROM=…`. Then `systemctl enable --now easl-site`; `journalctl -u easl-site` for logs. The server shuts down cleanly on SIGTERM.

Caddy (`/etc/caddy/Caddyfile`) proxies everything to it; the app sets its own cache headers (a year for `?v=` URLs, revalidation otherwise, `no-store` for pages that depend on the session), so the existing `Cache-Control` rules move out of Caddy:

```
easl-lang.org {
  encode zstd gzip
  reverse_proxy 127.0.0.1:8000
}

www.easl-lang.org {
  redir https://easl-lang.org{uri} permanent
}
```

(The app also compresses with gzip/brotli; Caddy leaves already-encoded responses alone. Either can be dropped.)

### Backups

The database is a single SQLite file in WAL mode, so copying it while the server runs can catch it mid-write. Use SQLite's own backup:

```sh
sqlite3 /var/lib/easl-site/easl-site.db ".backup '/var/backups/easl-site/easl-site-$(date +%F).db'"
```

from a daily cron job or systemd timer, copying the result off the machine (and pruning old ones). For continuous replication to object storage, [Litestream](https://litestream.io) runs as a sidecar with no changes to the app (`litestream replicate /var/lib/easl-site/easl-site.db s3://…`). Restoring is stopping the service and putting the file back.

## Decisions to review

- **Banned users' content is hidden, not just unlisted.** Their sketches are 404s (to everyone but admins), their profiles too, and their comments are left out of comment lists. The spec only asked to exclude them from listings; hiding everything seemed like what a ban is for. Unbanning restores it all.
- **Changing your email needs your password**, and invalidates every outstanding email link (so a link sent to the old address can't verify the new one).
- **A password reset also verifies the email** (following the link proves the address) and signs the browser in, after ending every other session.
- **Admins can't be banned** (revoke admin first), and can't ban themselves.
- **Likes don't need a verified email** (the spec lists publishing, commenting, and reporting). Likes drive "popular" and "trending", so throwaway accounts could game them; requiring verification is a one-line change in `api/sketches.rs`.
- **Views count once per viewer per hour** (by user, or by address when signed out), in memory, and never for the owner.
- **Counters are maintained by SQLite triggers** (`migrations/0001_init.sql`), so they stay right through cascading deletes (an account deleted with its likes and comments), which application code alone would miss.
- **Playlists** show each viewer only the items they can see (someone else's sketch going private disappears from the playlist for others). Reordering takes the ids the owner sees; items not mentioned keep their relative order after them. Adding an item twice is a no-op; a playlist holds at most 500. Adding someone's unlisted sketch to a public playlist shares its link, which is what unlisted means.
- **Forks** start private, copy the files and entry point but not the description, and show `forked_from` only while the viewer can see the original. Deleting the original keeps the forks.
- **Search** is a case-insensitive substring match (`LIKE`) on title, description, username, and display name. Fine at this scale; SQLite FTS5 is the upgrade path.
- **Signup says when a username or email is taken.** That reveals whether an email has an account (unavoidable without emailing a sign-up link first); sign-in and password-reset responses don't reveal it.
- **`display_name` in the API falls back to the username** when unset, so clients never show an empty name.
- **API responses include a few fields beyond the spec**: `hidden` on sketches (so owners can see a moderation hide), and `GET /api/playlists/<id>`.
- **`page_json` doesn't embed the examples**; the editor fetches `/playground/examples.json` itself.
- **Pages other than the editor send a strict Content-Security-Policy** (same-origin scripts, styles, images; no framing by other sites). The editor loads Monaco from jsdelivr, so it gets none; worth adding one tailored to it.
- **Thumbnails** of non-private sketches are cached publicly for a year under their `?v=` URL; private ones only by the viewer's browser. A sketch made private later can linger in a shared cache under its old URL (no CDN today, so moot for now).
- **Rate limits are in memory**, so they reset on restart, and they assume one server process.
- **Templates are embedded with `include_str!`** (one list in `src/templates.rs`; a template missing from it fails every test that renders it), and dev mode reads from `CARGO_MANIFEST_DIR`, so `--dev` is meant for `cargo run` in this directory.
- **`--origin` defaults to `http://<bind>`** rather than a fixed `http://127.0.0.1:8000`, so `--bind 127.0.0.1:8100` works without also passing `--origin`.
- **Reserved usernames** are in `src/validate.rs` (`RESERVED_USERNAMES`); worth a look before launch.

## Not done

- No email change confirmation to the *old* address, no "sign out everywhere" button, no 2FA.
- No moderation log (who hid or banned what, beyond `reports.resolved_by`) and no admin view of hidden sketches outside reports.
- No pagination on comments or a profile's playlists.
