# easl site server

The dynamic half of easl: the editor, accounts, sketches (multi-file easl programs people save and share), likes, comments, playlists, and moderation, as server-rendered pages and a JSON API, at `editor.easl-lang.org`. The docs stay at `easl-lang.org`, a static site (`../site`, built by `../generator`) that Caddy serves straight from disk in production.

The server routes each request by its `Host`: the editor origin's hostname (`--editor-origin`, compared case-insensitively, port ignored) gets the app; any other host gets the docs, which the server also serves so development needs one process. On the docs host, the old playground and the app's paths redirect to the editor host.

One Rust binary (axum + SQLite via sqlx + minijinja templates), built for a small VM (1 vCPU, 1 GB RAM).

## Running it in development

```sh
cd ../generator && cargo run        # build the static site into ../site
cd ../server
cargo run -- seed                   # users ella (admin), alice, bob, carol (password: password123), the examples, more
cargo run -- serve --dev            # docs at http://localhost:8000, the app at http://editor.localhost:8000
```

Browsers resolve `*.localhost` to the loopback address, so `http://editor.localhost:8000` needs no setup. For curl, pass the host: `curl -H 'Host: editor.localhost:8000' http://127.0.0.1:8000/` (or `curl http://editor.localhost:8000/` where curl resolves it too). With `--bind 127.0.0.1:8150`, the defaults follow the port (`http://editor.localhost:8150`, `http://localhost:8150`).

`--dev`:

- accepts `http://localhost:<port>`, `http://127.0.0.1:<port>`, and `http://editor.localhost:<port>` as request origins (besides `--editor-origin`),
- sets cookies without `Secure`, so sign-in works over plain http,
- reloads templates (`templates/`) and app assets (`static/`) from disk on every request, so design changes show on refresh,
- logs emails instead of sending them (unless SMTP is configured) and shows verification and reset links on the page in a "Dev mode" box.

`cargo run -- admin grant alice` makes a user an admin; it takes effect on their next request, and `/admin` appears in their account menu. Granting and revoking are recorded in the moderation log.

Tests: `cargo test` (integration tests in `tests/` run the real router against a fresh SQLite file each). `cargo clippy --all-targets` and `cargo fmt` should be clean.

## Commands and configuration

```
easl-site-server serve         [--bind 127.0.0.1:8000] [--site-dir ../site] [--db ./data/easl-site.db]
                               [--editor-origin http://editor.localhost:<port>] [--docs-origin http://localhost:<port>]
                               [--sync-examples-owner <username>] [--dev] [--trust-proxy]
easl-site-server sync-examples [--owner ella] [--db …] [--site-dir …]
easl-site-server seed          [--db …] [--site-dir …]
easl-site-server admin grant|revoke <username> [--db …]
```

- `--editor-origin` is the app's public origin (`https://editor.easl-lang.org`). Its hostname decides which requests get the app, and it's used for absolute links (emails, link previews, redirects from the docs) and the CSRF check, so it must match what browsers see. It defaults to `http://editor.localhost:<bind port>`.
- `--docs-origin` is the docs' public origin (`https://easl-lang.org`), for the app pages' links to the docs. It defaults to `http://localhost:<bind port>`.
- `--sync-examples-owner ella` runs the examples sync (below) at startup when that user exists, logging and carrying on otherwise (no such user, no `editor/examples.json`, a conflict), so each deploy of a rebuilt site keeps the examples current.
- `--trust-proxy` takes the client address (for rate limiting) from the last hop of `X-Forwarded-For`, which Caddy appends. Only set it behind a proxy, or clients can pick their own address.
- `SMTP_URL` (e.g. `smtps://user:pass@smtp.example.com:465`) and `MAIL_FROM` (e.g. `easl <noreply@easl-lang.org>`) enable email. Without them, emails are logged, with a warning at startup outside dev.
- `RUST_LOG` sets log levels (default `info,tower_http=info,sqlx=warn`).
- The database is created and migrated (`migrations/`) on startup. Expired sessions and email tokens are deleted hourly.

The editor pages read the editor build's versions from `<site-dir>/editor/manifest.json` (`{"version", "monaco_version"}`), and the docs' playground redirects read `<site-dir>/editor/examples.json`; both are re-read when the file changes, so a site rebuild needs no restart. Templates and app assets are compiled into the binary in release builds, so deploying the server means deploying one file.

### The examples

The docs' example programs are ordinary public sketches owned by `ella` (Ella's account), at ids fixed in the repo so the docs can link to them. The generator lists them in `<site-dir>/editor/examples.json` (`{"examples": [{"id", "sketch_id", "title", "description", "files": [{"path", "content"}]}]}`; `id` is the old example name that `/playground?example=<id>` links use). `sync-examples` makes each one a sketch with exactly its `sketch_id`, owned by `--owner`: it creates a missing one (public, published now), and updates an existing one's title, description, and files when they differ (bumping `updated_at` only then; the entry file stays unless it's no longer one of the files). New files are saved as a new revision and published at once, replacing any draft of Ella's (the draft stays in the history); a draft identical to the example is just published. It never deletes a sketch and never touches likes, comments, thumbnails, tags, or visibility (so a hidden or unlisted example stays that way). Everything is checked before anything is written: it fails when the owner doesn't exist, when an example is invalid or two share a sketch id, or when a sketch id belongs to someone else's sketch. Without `editor/examples.json` it says so and does nothing. Running it again changes nothing.

## How it's organized

| | |
|---|---|
| `src/main.rs` | the CLI |
| `src/app.rs` | routing by host, the editor host's router, and the layers every request passes through |
| `src/docs.rs` | the docs host: the static site and its redirects to the editor host |
| `src/config.rs`, `src/state.rs` | configuration, rate limits, shared state |
| `src/db.rs`, `migrations/` | the pool (WAL, foreign keys, 4 connections), ids, the schema |
| `src/auth.rs` | passwords (argon2id), sessions, email tokens, the session middleware and `MaybeUser`/`ApiUser`/`PageUser` extractors |
| `src/security.rs` | the CSRF check and client addresses |
| `src/ratelimit.rs` | in-memory sliding-window limits |
| `src/users.rs`, `sketches.rs`, `playlists.rs`, `comments.rs`, `reports.rs` | the data layer and the API's JSON shapes |
| `src/moderation.rs` | admin actions and the moderation log, and the admin pages' lists |
| `src/revisions.rs` | sketch revisions: every saved version of a sketch's program |
| `src/tags.rs` | sketch tags, and the most-used ones |
| `src/visibility.rs` | who can see what (one rule, used everywhere) |
| `src/api/` | `/api/…` handlers |
| `src/pages/` | page handlers (`editor.rs` renders `editor.html` for `/new` and `/s/<id>`) |
| `src/templates.rs`, `templates/` | minijinja (autoescaped); `base.html` is the layout, `macros.html` the sketch card |
| `src/assets.rs`, `static/` | the pages' own CSS/JS at `/static/…?v=<hash>` |
| `src/site.rs` | serving site files, and the editor build's manifest and example index |
| `src/examples.rs` | `sync-examples`: the docs' examples as sketches |
| `src/avatar.rs` | generated SVG avatars and placeholder thumbnails |
| `src/text.rs` | plain text → HTML (escape, line breaks, links), dates |
| `src/mail.rs` | SMTP / log / test mailers |
| `src/validate.rs` | every input rule |
| `src/seed.rs` | demo data |

### The editor template's context

`editor.html` gets `page_json` (already safe inside `<script type="application/json">`), `asset_version`, `monaco_version`, `page_title`, `og` (`title`, `description`, `image`, `url`; absolute URLs on the editor origin), plus what every template gets: `me`, `path`, `login_url`, `dev`, `editor_origin`, `docs_origin`, `app_base` (what the app's links start with: empty, or the editor origin on a 404 page served on the docs host), and the `static(path)` function. `page_json` is `{"mode": "new" | "sketch", "user": Me | null, "sketch": Sketch | null, "playlist": {id, title, index, count, prev, next} | null, "editor_origin", "docs_origin", "csrf_ok": true}`.

### Drafts and publishing

A sketch's program (its files and entry point) lives in its **revisions** (`sketch_revisions`, `revision_files`), numbered from 1 per sketch and kept forever. Each save of different files or a different entry point (`PUT /api/sketches/<id>` with `files`, `entry_file`, or `entry_name`) stores the whole program as a new revision numbered after the highest so far and makes it the sketch's **head**; a save identical to the head stores nothing. Title, description, and tags aren’t versioned.

While a sketch is unlisted or public it also has a **published** revision, which is what everyone but its owner sees (`GET /api/sketches/<id>`, the editor's `page_json.sketch`, forks, listings' entry points); its owner sees the head. A private sketch has no published revision (an admin looking at one sees its head). Changing the visibility from private to unlisted or public publishes the head; between unlisted and public the published revision stays; making it private unpublishes it. Saving to an unlisted or public sketch moves only the head, so it **has a draft** while the head differs from the published revision. `POST …/publish` makes the head published (400 for a private sketch); `POST …/discard-draft` moves the head back to the published revision (400 without a draft), keeping the discarded revisions in the history.

The sketch JSON carries `head_revision`, `published_revision` (or null), `has_draft` (false for anyone but the owner), `draft_saved_at` (when the head was saved, for the owner; null for others), and `published_updated_at` (when the published revision last changed). `updated_at` is the last change of any kind and `published_at` when the sketch first became public. Others see a profile's sketches ordered by `published_updated_at`, so saving a draft doesn't move a card for them; the owner's own profile orders by `updated_at`. Explore's "new" stays ordered by `published_at`.

The migration (`0002_revisions.sql`) turned each existing sketch's files into its revision 1, published unless the sketch was private.

### Tags

A sketch has up to 8 **tags**, set by its owner and kept in the order given, stored in `sketch_tags (sketch_id, tag, position)` (`migrations/0003_tags.sql`, indexed by tag). Like the title and description they apply at once and aren't part of revisions or drafts. A tag is 1–32 characters of lowercase ASCII letters, digits, and `-`, not starting or ending with `-`; input is normalized first (trimmed, lowercased, a leading `#` dropped, each run of whitespace, `_`, and `-` turned into one `-`), so `#Ray Marching` is `ray-marching`, and a sketch's tags must be distinct after that (`validate::tag`, `validate::tags`).

The sketch JSON (and the summaries in listings) has `tags: [string]`. `POST /api/sketches` and `PUT /api/sketches/<id>` take `tags`; on a `PUT` it replaces the set (owner only; leaving it out keeps the tags), and a bad tag is a 400 naming it and the rule. Changing the tags bumps `updated_at` like a title change. Forks start with the original's tags.

Explore takes `?tag=<tag>` (normalized the same way): only listed sketches with that tag, under a "Tagged #tag" heading with a link back to all sketches; the sort tabs, search form, and pages keep it. Above the grid is a row of the 16 tags on the most listed sketches (ties by name, counting only what's listed: public, not hidden, owner not banned), each linking to its filter with the sort and search kept, the current one highlighted and linking back to all. Cards show their first 3 tags as links.

### Moderation

Admins (`admin grant`, below) moderate from `/admin`. Every admin action goes through `src/moderation.rs`, which makes the change and records it in the **moderation log** (`moderation_actions`, `migrations/0004_moderation_log.sql`) in one transaction: hiding and unhiding a sketch, deleting someone else's sketch, deleting a comment on someone else's sketch by someone else, restoring a comment, banning and unbanning, resolving a report, and granting or revoking admin rights from the command line (logged with no admin and the note `command line`). A row holds the admin (NULL once their account is deleted), the action, the target's kind and id (sketch id, comment id, username, report id), and what the log shows of the target as it was then (the user it is or belongs to, the sketch it is or is on and its title, a 200-character comment excerpt), so it stays readable after deletions. People acting on their own content (an author deleting their comment, an owner deleting their sketch or a comment on it) aren't moderating and aren't logged, admins included. Actions that change nothing (hiding a hidden sketch, banning a banned user, resolving a resolved report) aren't logged, and keep the original `hidden_at`/`banned_at`/`resolved_at`.

Comments are soft-deleted (`deleted_at`) and record who deleted them (`deleted_by`, NULL for deletions made before it was recorded or by an account since deleted), so the admin pages tell the author, the sketch's owner, and a moderator apart. `POST /api/admin/comments/<id>/restore` undoes a deletion (404 for an unknown comment, 400 when it isn't deleted); a trigger counts the comment again. The migration put the reports already resolved into the log (who resolved each and when, with its target as it was at migration time); earlier bans, hides, and deletions weren't recorded, so the pages say "unknown" for who did them.

`/admin` has tabs (`?tab=reports|banned|hidden|comments|log`, with the Quick actions panel above them, counts in the labels):

- **Reports**: open reports, oldest first, with their actions; `?resolved=1` lists resolved ones, newest first, with who resolved them and when.
- **Banned users**: newest ban first, with who banned them (from the log), their sketch and comment counts, and Unban.
- **Hidden sketches**: newest first, with the owner, who hid it, and Unhide.
- **Deleted comments**: newest first, with the sketch, the author, and who deleted it ("the author", "the sketch owner", "moderator @name", or "unknown"), and Restore.
- **Log**: every action newest first, as sentences ("@ella banned @bob", "@ella deleted a comment by @bob on “…”").

Every list but the open reports pages by 50. Usernames on the admin pages link to users' moderation pages; a display name on the Banned tab, and each moderation page, link to the public profile.

**A user's moderation page**, `/admin/users/<username>` (`?list=submitted|against|actions|comments&page=`), is the "moderator mode" view of an account, kept apart from the profile so admins browsing the site see what everyone sees: under a "Moderation view" banner, the account's facts (its **email address**, as a `mailto:` link so moderators can follow up by email, whether it's verified, when it joined, admin status, and ban status with who banned it and a Ban/Unban button, which admins' pages don't offer), its sketch count with how many are hidden or private, its comment and deleted-comment counts, the reports it submitted and the reports against it (of the user, their sketches, or their comments), each with its status, the log entries involving it, and its deleted comments. The overview shows the newest 10 of each list with a "Show all" link to that list alone, paged by 50. Profiles show admins a small shield icon beside the name linking there, and the Reports tab links each reporter's and reported user's page. Email addresses appear nowhere else (no other page and no API response shows anyone's).

### Routes

On the editor host — pages: `/` (explore: `?sort=new|popular|trending&q=&tag=&page=`; `/explore` redirects there with its query), `/u/<username>` (`?tab=sketches|playlists|likes&page=`), `/u/<username>/avatar.svg`, `/p/<id>`, `/playlists/new` (the form; it posts back to itself and goes to the new playlist), `/s/<id>` (`?playlist=`), `/s/<id>/thumb`, `/new`, `/login`, `/signup`, `POST /logout`, `/forgot`, `/reset/<token>`, `/verify/<token>`, `/settings` (forms post to `/settings/profile|email|password|verification|delete`), `/admin` (`?tab=reports|banned|hidden|comments|log&resolved=1&page=`), `/admin/users/<username>` (`?list=submitted|against|actions|comments&page=`). Files: `/static/…` (the app's assets), and from the site directory only `/editor/…` and `/easl-web/…` (`?v=` URLs cached for a year, others revalidated). Anything else is a 404.

On the docs host: `/playground` and `/playground.html` redirect (308) to `<editor-origin>/s/<sketch id>` for a known `?example=<id>`, else to `<editor-origin>/new`; paths whose first segment is one of the app's (`explore`, `new`, `s`, `u`, `p`, `playlists`, `login`, `signup`, `logout`, `forgot`, `reset`, `verify`, `settings`, `admin`, `api`) redirect (308) to the same path and query on the editor host. Everything else is the static site.

API (`/api`): `GET me`, `GET me/sketches`, `GET me/playlists`; `POST sketches`; `GET|PUT|DELETE sketches/<id>` (`PUT` takes any of `title`, `description`, `files`, `entry_file`, `entry_name`, `visibility`, `tags`); `POST sketches/<id>/publish`, `POST sketches/<id>/discard-draft` (owner); `GET sketches/<id>/published` (`{revision, files, entry_file, entry_name, published_at}`, 404 when nothing is published); `GET sketches/<id>/revisions` (owner; `{revisions: [{number, created_at, published, head}]}`, newest first); `POST sketches/<id>/fork`; `PUT sketches/<id>/thumbnail`; `POST|DELETE sketches/<id>/like`; `GET|POST sketches/<id>/comments`; `DELETE comments/<id>`; `POST playlists`; `GET|PUT|DELETE playlists/<id>`; `POST playlists/<id>/items`; `DELETE playlists/<id>/items/<sketch_id>`; `PUT playlists/<id>/order`; `POST reports`; `POST admin/sketches/<id>/hide|unhide`, `admin/users/<username>/ban|unban`, `admin/reports/<id>/resolve`, `admin/comments/<id>/restore`. Errors are `{"error": "…"}` with 400/401/403/404/409/413/429.

## Production

Two hostnames, one machine: Caddy serves `easl-lang.org` (the docs) from files, and reverse-proxies `editor.easl-lang.org` to this server. DNS needs a record for `editor` pointing at the same machine as the apex: an `A` (and `AAAA`, if the apex has one) record for `editor.easl-lang.org` with the same addresses, or a `CNAME` from `editor` to `easl-lang.org`. Caddy gets the new name's certificate by itself once the record resolves.

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
  --editor-origin https://editor.easl-lang.org \
  --docs-origin https://easl-lang.org \
  --sync-examples-owner ella \
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

Caddy (`/etc/caddy/Caddyfile`) serves the docs itself and proxies the editor host. The app sets its own cache headers (a year for `?v=` URLs, revalidation otherwise, `no-store` for pages that depend on the session); the docs keep Caddy's rules. On the docs host, Caddy hands the old playground and the app's paths to the server, which answers them with its docs-host redirects (it maps `?example=<id>` to the example's sketch, so the mapping lives in one place):

```
easl-lang.org {
  encode zstd gzip

  # The old playground and the app's paths: the server redirects them to
  # the editor host.
  @moved path /playground /playground.html /explore /explore/* /new /s/* /u/* /p/* /playlists/* /login /signup /logout /forgot /reset/* /verify/* /settings /settings/* /admin /admin/* /api /api/*
  handle @moved {
    reverse_proxy 127.0.0.1:8000
  }

  handle {
    root * /srv/easl-site
    file_server
    # (the existing Cache-Control rules for the docs stay here)
  }
}

editor.easl-lang.org {
  encode zstd gzip
  reverse_proxy 127.0.0.1:8000
}

www.easl-lang.org {
  redir https://easl-lang.org{uri} permanent
}
```

Caddy passes the original `Host` through, which is how the server tells the two hosts apart: requests proxied from the `easl-lang.org` block arrive as `Host: easl-lang.org` and get the docs host's redirects. (The app also compresses with gzip/brotli; Caddy leaves already-encoded responses alone. Either can be dropped.)

The examples: deploy the site first, then the server. With `--sync-examples-owner ella`, each start syncs `editor/examples.json` into ella's sketches (and logs and carries on when ella doesn't exist yet). To sync by hand after a site deploy without restarting: `sudo -u easl easl-site-server sync-examples --db /var/lib/easl-site/easl-site.db --site-dir /srv/easl-site`. **Create the `ella` account (sign up at the editor host) before announcing the site**: whoever registers `ella` first owns the examples.

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
- **Liking needs a verified email**, like publishing, commenting, and reporting: likes drive "popular" and "trending", so throwaway accounts could otherwise game them. Unliking doesn't.
- **Views count once per viewer per hour** (by user, or by address when signed out), in memory, and never for the owner.
- **Counters are maintained by SQLite triggers** (`migrations/0001_init.sql`), so they stay right through cascading deletes (an account deleted with its likes and comments), which application code alone would miss.
- **Playlists** show each viewer only the items they can see (someone else's sketch going private disappears from the playlist for others). Reordering takes the ids the owner sees; items not mentioned keep their relative order after them. Adding an item twice is a no-op; a playlist holds at most 500. Adding someone's unlisted sketch to a public playlist shares its link, which is what unlisted means.
- **Forks** start private, copy the files and entry point the forker sees (the published revision, or the head of their own sketch) and the tags but not the description, and show `forked_from` only while the viewer can see the original. Deleting the original keeps the forks.
- **Search** is a case-insensitive substring match (`LIKE`) on title, description, username, display name, and tags, where the query is first normalized as a tag (so `#Ray Marching` and `ray_march` find `ray-marching`; a query that normalizes to nothing, like `#`, matches no tag). Fine at this scale; SQLite FTS5 is the upgrade path.
- **Signup says when a username or email is taken.** That reveals whether an email has an account (unavoidable without emailing a sign-up link first); sign-in and password-reset responses don't reveal it.
- **`display_name` in the API falls back to the username** when unset, so clients never show an empty name.
- **API responses include a few fields beyond the spec**: `hidden` on sketches (so owners can see a moderation hide), and `GET /api/playlists/<id>`.
- **`/explore` redirects to `/`** (308, query kept), so the gallery has one URL.
- **The docs host's 404 page** is the app's error page with absolute links to the editor host.
- **New playlists are made by a server-side form** (`/playlists/new`, linked from the profile's Playlists tab, its empty state, playlist pages, and the account menu) rather than by script calling the JSON API: it works without JavaScript and reports mistakes on the form. It uses the same data layer and rules as `POST /api/playlists`. The form defaults to public for verified users and private for others.
- **A `PUT` that changes nothing** (identical files, same title, …) leaves `updated_at` alone.
- **`head_revision` and `published_revision` are the real pointers for everyone**, so a viewer comparing them can tell an unpublished draft exists (as `updated_at` already shows when the owner last saved); only `has_draft` and `draft_saved_at` are owner-only.
- **Listings break `updated_at` ties by insertion order** (newest first), not by id, so two sketches saved in the same second list in the order they were made.
- **Sessions are host-only** on the editor host (no `Domain`), so the docs never see the cookie.
- **Pages other than the editor send a strict Content-Security-Policy** (same-origin scripts, styles, images; no framing by other sites). The editor loads Monaco from jsdelivr, so it gets none; worth adding one tailored to it.
- **Thumbnails** of non-private sketches are cached publicly for a year under their `?v=` URL; private ones only by the viewer's browser. A sketch made private later can linger in a shared cache under its old URL (no CDN today, so moot for now).
- **Rate limits are in memory**, so they reset on restart, and they assume one server process.
- **Tags are free-form**, with no curated list or moderation beyond hiding a sketch; the tag row shows whatever listed sketches use most.
- **`seed` tags the examples that have no tags** each time it runs (so an existing database gets them too), and tags the other demo users' sketches when it makes them; on a database seeded before tags existed, those stay untagged.
- **Templates are embedded with `include_str!`** (one list in `src/templates.rs`; a template missing from it fails every test that renders it), and dev mode reads from `CARGO_MANIFEST_DIR`, so `--dev` is meant for `cargo run` in this directory.
- **The origins default from the bind port** (`http://editor.localhost:<port>`, `http://localhost:<port>`), so `--bind 127.0.0.1:8150` works without passing either.
- **The moderation log's `action` and `target_kind` aren't `CHECK`ed in SQL** (unlike `reports.kind`), so a new kind of action needs no table rebuild; `moderation::Action` lists them.
- **An admin deleting a comment counts as moderating only when it's neither their comment nor on their sketch**; as an author or owner they're logged like anyone else (not at all), and the Deleted comments tab labels them "the author" or "the sketch owner".
- **Resolved reports show no actions** on the Reports tab (nothing reopens a report).
- **A user's moderation page** shows the log entries *about* them, not the ones they took as an admin (the Log tab has those).
- **Reserved usernames** are in `src/validate.rs` (`RESERVED_USERNAMES`; `editor` and `playground` are among them); worth a look before launch. `ella` isn't reserved, since that would stop Ella signing up as `ella`; see the production note about registering it first.

## Not done

- No email change confirmation to the *old* address, no "sign out everywhere" button, no 2FA.
- No pagination on comments or a profile's playlists.
