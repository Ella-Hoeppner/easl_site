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
                               [--min-free-disk 2G] [--geoip-db <path>]
easl-site-server sync-examples [--owner ella] [--db …] [--site-dir …]
easl-site-server seed          [--db …] [--site-dir …]
easl-site-server admin grant|revoke <username> [--db …]
easl-site-server visits        [--days 30] [--db …]
```

- `--editor-origin` is the app's public origin (`https://editor.easl-lang.org`). Its hostname decides which requests get the app, and it's used for absolute links (emails, link previews, redirects from the docs) and the CSRF check, so it must match what browsers see. It defaults to `http://editor.localhost:<bind port>`.
- `--docs-origin` is the docs' public origin (`https://easl-lang.org`), for the app pages' links to the docs. It defaults to `http://localhost:<bind port>`.
- `--sync-examples-owner ella` runs the examples sync (below) at startup when that user exists, logging and carrying on otherwise (no such user, no `editor/examples.json`, a conflict), so each deploy of a rebuilt site keeps the examples current.
- `--trust-proxy` takes the client address (for rate limiting) from the last hop of `X-Forwarded-For`, which Caddy appends. Only set it behind a proxy, or clients can pick their own address.
- `--min-free-disk` (default `2G`; a number of bytes with an optional `K`/`M`/`G`/`T` unit, binary; `0` turns it off) is the free space the database's filesystem must keep: below it, the server refuses to store sketch content (see "Free disk").
- `--geoip-db` is the GeoIP database visits' countries are looked up in (see "Visits"); without it, visits are recorded with no country.
- `SMTP_URL` (e.g. `smtps://user:pass@smtp.example.com:465`) and `MAIL_FROM` (e.g. `easl <noreply@easl-lang.org>`) enable email. Without them, emails are logged, with a warning at startup outside dev.
- `RUST_LOG` sets log levels (default `info,tower_http=info,sqlx=warn`).
- The database is created and migrated (`migrations/`) on startup. Expired sessions and email tokens, and daily email counts older than 30 days, are deleted hourly; the same hourly upkeep drops a past day's visitor salt and rereads the GeoIP database if its file changed.

The editor pages read the editor build's versions from `<site-dir>/editor/manifest.json` (`{"version", "monaco_version"}`; the page loads Monaco from the site's `/editor/monaco/<monaco_version>/`), and the docs' playground redirects read `<site-dir>/editor/examples.json`; both are re-read when the file changes, so a site rebuild needs no restart. Templates and app assets are compiled into the binary in release builds, so deploying the server means deploying one file.

### The examples

The docs' example programs are ordinary public sketches owned by `ella` (Ella's account), at ids fixed in the repo so the docs can link to them. The generator lists them in `<site-dir>/editor/examples.json` (`{"examples": [{"id", "sketch_id", "title", "description", "files": [{"path", "content"}]}]}`; `id` is the old example name that `/playground?example=<id>` links use). `sync-examples` makes each one a sketch with exactly its `sketch_id`, owned by `--owner`: it creates a missing one (public, published now), and brings over to an existing one only what the docs changed since its last sync, which `example_syncs` (`migrations/0007_example_syncs.sql`) remembers: a changed title or description replaces the sketch's, and changed files are saved as a new revision and published at once (bumping `updated_at` only then; the entry file stays unless it's no longer one of the files), replacing any draft of Ella's (the draft stays in the history); a draft identical to the new files is just published. Whatever the docs didn't change stays as Ella edited it on the site, draft included, so a restart never undoes her edits. A sketch with no `example_syncs` row (synced before they were kept) is adopted as it is: nothing about it changes, and the docs' current version is remembered. It never deletes a sketch and never touches likes, comments, thumbnails, tags, or visibility (so a hidden or unlisted example stays that way). Everything is checked before anything is written: it fails when the owner doesn't exist, when an example is invalid or two share a sketch id, or when a sketch id belongs to someone else's sketch. Without `editor/examples.json` it says so and does nothing. Running it again changes nothing.

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
| `src/ratelimit.rs` | in-memory sliding-window limits (see "Limits") |
| `src/outbox.rs` | sending email within the site's daily budget |
| `src/disk.rs` | the free-disk guard |
| `src/visits.rs` | counting page views (see "Visits") |
| `src/users.rs`, `sketches.rs`, `playlists.rs`, `comments.rs`, `reports.rs` | the data layer and the API's JSON shapes |
| `src/moderation.rs` | admin actions and the moderation log, and the admin pages' lists |
| `src/revisions.rs` | sketch revisions: the saved versions of a sketch's program, and pruning them to the history budget |
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

A sketch's program (its files and entry point) lives in its **revisions** (`sketch_revisions`, `revision_files`), numbered from 1 per sketch and kept within the sketch's history budget (below). Each save of different files or a different entry point (`PUT /api/sketches/<id>` with `files`, `entry_file`, or `entry_name`) stores the whole program as a new revision numbered after the highest so far and makes it the sketch's **head**; a save identical to the head stores nothing. Title, description, and tags aren’t versioned.

While a sketch is unlisted or public it also has a **published** revision, which is what everyone but its owner sees (`GET /api/sketches/<id>`, the editor's `page_json.sketch`, forks, listings' entry points); its owner sees the head. A private sketch has no published revision (an admin looking at one sees its head). Changing the visibility from private to unlisted or public publishes the head; between unlisted and public the published revision stays; making it private unpublishes it. Saving to an unlisted or public sketch moves only the head, so it **has a draft** while the head differs from the published revision. `POST …/publish` makes the head published (400 for a private sketch); `POST …/discard-draft` moves the head back to the published revision (400 without a draft), keeping the discarded revisions in the history (until pruned).

The sketch JSON carries `head_revision`, `published_revision` (or null), `has_draft` (false for anyone but the owner), `draft_saved_at` (when the head was saved, for the owner; null for others), and `published_updated_at` (when the published revision last changed). `updated_at` is the last change of any kind and `published_at` when the sketch first became public. Others see a profile's sketches ordered by `published_updated_at`, so saving a draft doesn't move a card for them; the owner's own profile orders by `updated_at`. Explore's "new" stays ordered by `published_at`.

The migration (`0002_revisions.sql`) turned each existing sketch's files into its revision 1, published unless the sketch was private.

### Revision history budget

Each sketch's history has a size budget, 10 MB (`config::SKETCH_HISTORY_BYTES`), and each account's sketches' histories together have one of 5 MB while its email address is unverified (`config::UNVERIFIED_ACCOUNT_HISTORY_BYTES`) and 50 MB once it's verified (`config::VERIFIED_ACCOUNT_HISTORY_BYTES`); all three are `Config::history`. A revision's **size** (`revisions::size`, stored in `sketch_revisions.size`) is its files' paths and contents in bytes plus 64 bytes per file and 256 per revision for the rows holding them; each sketch keeps its revisions' total in `sketches.history_bytes`, maintained by triggers (like the like and comment counts), so a save checks both budgets without reading any files. Migration `0005_history_budget.sql` measured existing revisions the same way.

- **Pruning:** a save that stores a new revision and takes the sketch's history past its budget deletes the sketch's oldest revisions (lowest numbers first), in the same transaction, until it fits. It never deletes the head or the published revision, so a sketch can go over its budget only by those two (at most 2 MB, since a save holds at most 1 MB). A short program keeps thousands of versions; one near the 1 MB limit keeps about ten.
- **The account budget:** when, even after that pruning, a save or a new sketch (including a fork) would take the owner's total past their budget, nothing is saved (pruning included) and the request fails with 413: "… would take the saved versions of all your sketches past 50 MB, the most an account can keep. Delete sketches you no longer need to make room. …", or, for an unverified account, "… past 5 MB, the most an account can keep until its email address is verified. Verify your email address to raise the limit to 50 MB, or delete sketches you no longer need to make room." A save that doesn't grow the total (pruning freed as much as it added) is allowed even over the budget. Unverified accounts get little because they're cheap to make (5 sign-ups an hour per address); verifying takes an email, and the site sends at most 80 a day, so verified accounts can't be made in bulk. An account whose email changes is unverified again, with the smaller budget, until it verifies the new address.
- **Numbering:** a new revision is numbered after the highest so far. The highest is never pruned (pruning happens only when it's saved, and then it's the head), so numbers are never reused. `GET …/revisions` lists what's left, so numbers can have gaps.
- **Reading:** everything that reads a sketch's head or published revision (the sketch JSON, `GET …/published`, forks, a save's merge with the head) reads the sketch row and the revision in one transaction, rereading the row when a concurrent save has pruned the revision it named.
- The examples sync and `seed` use the default budgets.

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

### Limits

**Rate limits** (`config::RateLimits`, in memory, `src/ratelimit.rs`). Each is its own bucket, so hitting one never blocks another action; an action with two limits (hourly and daily) is refused when either is reached, and a refused attempt counts against neither. Exceeding one is a 429 ("Too many requests…", on the page's form for page routes).

| What | Per | Limit |
|---|---|---|
| Sign-in attempts and password-reset form submissions | address | 10 per 10 minutes |
| Sign-ups (each sends a verification email) | address | 5 per hour, counting attempts that pass validation and name a free username and email |
| Password-reset requests (`/forgot`) | address | 10 per hour |
| Password-reset emails | recipient account | 3 per hour and 6 per day; silent (see below) |
| Verification emails after sign-up (`/settings/verification`, and changing the email) | account | 3 per hour and 10 per day |
| Current-password checks in settings (`/settings/email`, `/settings/password`, `/settings/delete`) | account | 10 per 10 minutes, checked before hashing |
| Sketches created, including forks | account | 30 per hour and 100 per day |
| Playlists created (API and form) | account | 20 per hour |
| Comments | account | 10 per minute |
| Reports | account | 10 per hour |
| Page views recorded (see "Visits") | address | 300 per hour; past it, views go uncounted (silently) |

**Email.** Resend's free tier sends 100 emails a day, so the site sends at most 80 a day (`config::DAILY_EMAIL_BUDGET`, `Config::daily_email_budget`; UTC days), leaving room for test emails. Every email goes through `outbox::send`, which counts it in the database (`email_days`, one row per day), so a restart doesn't reset the count; with the day's budget spent it sends nothing, logs a warning, and the user sees "We can't send email right now. Please try again later." (503):

- **Sign-up** still creates the account and signs in, and the page says the verification email couldn't be sent and can be sent from settings later.
- **Resending verification** shows the message in the email section; **changing the email** is refused with it before the address changes.
- **`/forgot`** shows it for every address, with or without an account, so it reveals nothing about which addresses have accounts. When the recipient's own reset limit is reached, it shows the usual "If an account uses …, we've sent it a link" page and sends nothing, exactly as for an unknown address.

**Free disk.** The last backstop (`src/disk.rs`): before storing sketch content or a thumbnail (`POST /api/sketches`, a fork, a `PUT` that saves files or the entry point, `PUT …/thumbnail`), the server checks the free space on the database's filesystem (`statvfs`, through the `fs4` crate, measured at most every 5 seconds) and refuses with 507 "The site is out of storage right now, so it can't save sketches or thumbnails. Your work hasn't been saved; please try again later." while it's below `--min-free-disk` (2 GB; `Config::min_free_disk`, measured on `Config::data_dir`). Each measurement below the threshold logs a warning, so a full disk shows in the log every few seconds while it lasts. Everything else (reading, title and visibility changes, comments, sign-ins) keeps working. When the space can't be measured, it logs the error and lets writes through. The budgets bound each account; this bounds them all together.

**Password-reset links.** Only the newest works: asking for a reset deletes the account's earlier reset tokens. Changing the password in settings deletes them too, in the transaction that changes the hash (and ends the other sessions), as using a reset link and changing the email already did.

### Visits

The server counts page views on both hosts (`src/visits.rs`, `migrations/0006_visits.sql`), to show how much the site is used and from where in the world. Every page loads `visit.js` (the app's `static/visit.js`, and the docs' `generator/assets/visit.js`, which the generator copies into the site and every docs page loads), which sends `POST /visit` with `navigator.sendBeacon`. Only browsers that run the page's scripts send it, which leaves out most crawlers. A beacon is recorded, as one row of `visits`, when its `Origin` is one of the site's (the docs origin, or one the app accepts), its user agent is present and has none of `bot`, `crawl`, `spider`, `slurp`, or `headless` in it, its address is within its limit (300 an hour), and the free-disk guard allows writes. Whether it's recorded or not, the answer is 204. The CSRF check lets `/visit` through, since the docs pages send it from the docs origin, which the check doesn't accept.

A row holds:

- `at` (unix seconds) and `site` (`docs` or `editor`);
- `country`: the address's ISO 3166-1 code from the GeoIP database, NULL without one or when it doesn't know the address (such as local addresses in development);
- `visitor`: the first 8 bytes, in hex, of a SHA-256 of a salt, the address, and the user agent. The salt is 32 random bytes kept only in memory and replaced each UTC day (the hourly upkeep drops a past day's), so a browser has one hash a day across both hosts. Once the day is over, its hashes can't be tied to an address, or to the same browser on another day.

The address itself is never written anywhere. A restart makes a new salt, so a visitor seen both before and after a restart counts twice that day. Rows (about 60 bytes each) are kept indefinitely.

`easl-site-server visits [--days 30]` prints the views and unique visitors by day (with each host's views) and by country, where a visitor counts once a day; on the machine, `sudo -u easl easl-site-server visits --db /var/lib/easl-site/easl-site.db`.

**The GeoIP database** is DB-IP's free [IP to Country Lite](https://db-ip.com/db/download/ip-to-country-lite) in MaxMind's `.mmdb` format, about 8.5 MB, read into memory (`--geoip-db`; the server rereads it within an hour of the file changing). It's licensed CC BY 4.0, so showing its data publicly (a map of visitors, say) needs DB-IP's attribution: "IP Geolocation by DB-IP", linking to https://db-ip.com. To try countries in development, download it (`EASL_SITE_GEOIP_DB=./data/dbip-country-lite.mmdb deploy/easl-site-geoip`) and pass `--geoip-db ./data/dbip-country-lite.mmdb` with `--trust-proxy`, sending beacons with an `X-Forwarded-For` address, since local addresses have no country.

### Routes

On the editor host — pages: `/` (explore: `?sort=new|popular|trending&q=&tag=&page=`; `/explore` redirects there with its query), `/u/<username>` (`?tab=sketches|playlists|likes&page=`), `/u/<username>/avatar.svg`, `/p/<id>`, `/playlists/new` (the form; it posts back to itself and goes to the new playlist), `/s/<id>` (`?playlist=`), `/s/<id>/thumb`, `/new`, `/login`, `/signup`, `POST /logout`, `/forgot`, `/reset/<token>`, `/verify/<token>`, `/settings` (forms post to `/settings/profile|email|password|verification|delete`), `/admin` (`?tab=reports|banned|hidden|comments|log&resolved=1&page=`), `/admin/users/<username>` (`?list=submitted|against|actions|comments&page=`). `POST /visit` (the visit beacon). Files: `/static/…` (the app's assets), and from the site directory only `/editor/…` and `/easl-web/…` (`?v=` URLs cached for a year, others revalidated). Anything else is a 404.

On the docs host: `/playground` and `/playground.html` redirect (308) to `<editor-origin>/s/<sketch id>` for a known `?example=<id>`, else to `<editor-origin>/new`; paths whose first segment is one of the app's (`explore`, `new`, `s`, `u`, `p`, `playlists`, `login`, `signup`, `logout`, `forgot`, `reset`, `verify`, `settings`, `admin`, `api`) redirect (308) to the same path and query on the editor host. `POST /visit` is the visit beacon. Everything else is the static site.

API (`/api`): `GET me`, `GET me/sketches`, `GET me/playlists`; `POST sketches`; `GET|PUT|DELETE sketches/<id>` (`PUT` takes any of `title`, `description`, `files`, `entry_file`, `entry_name`, `visibility`, `tags`); `POST sketches/<id>/publish`, `POST sketches/<id>/discard-draft` (owner); `GET sketches/<id>/published` (`{revision, files, entry_file, entry_name, published_at}`, 404 when nothing is published); `GET sketches/<id>/revisions` (owner; `{revisions: [{number, created_at, published, head}]}`, newest first); `POST sketches/<id>/fork`; `PUT sketches/<id>/thumbnail`; `POST|DELETE sketches/<id>/like`; `GET|POST sketches/<id>/comments`; `DELETE comments/<id>`; `POST playlists`; `GET|PUT|DELETE playlists/<id>`; `POST playlists/<id>/items`; `DELETE playlists/<id>/items/<sketch_id>`; `PUT playlists/<id>/order`; `POST reports`; `POST admin/sketches/<id>/hide|unhide`, `admin/users/<username>/ban|unban`, `admin/reports/<id>/resolve`, `admin/comments/<id>/restore`. Errors are `{"error": "…"}` with 400/401/403/404/409/413/429/503/507.

## Production

Two hostnames, one machine: Caddy serves `easl-lang.org` (the docs) from files, and reverse-proxies `editor.easl-lang.org` to this server. DNS needs a record for `editor` pointing at the same machine as the apex: an `A` (and `AAAA`, if the apex has one) record for `editor.easl-lang.org` with the same addresses, or a `CNAME` from `editor` to `easl-lang.org`. Caddy gets the new name's certificate by itself once the record resolves.

The files for the machine are in `deploy/`: the service (`easl-site.service`), Caddy's config (`Caddyfile`), the backup job (see "Backups"), the GeoIP database's update job (see "The GeoIP database" below), and `easl-site-mail-test`, which sends a test email with the server's settings. The repo's `deploy.sh` deploys through the `easl-site` SSH host: `./deploy.sh site` builds the docs from scratch and uploads them to `/srv/easl-site` (as the `deploy` user); `./deploy.sh server` uploads this directory's source to `~deploy/easl-site-server`, builds it there (`cargo build --release --locked`, a couple of minutes), installs `/usr/local/bin/easl-site-server`, and restarts the service if it's enabled; `./deploy.sh` does both, site first (the server syncs the examples from it when it starts). Templates and app assets are compiled into the binary, so the server is one file.

The machine (Ubuntu): Caddy from its apt repository; `build-essential`, `pkg-config`, `rsync`, and `sqlite3`; Rust (rustup's minimal profile) for the `deploy` user; a system user `easl` that owns the database in `/var/lib/easl-site/` (systemd makes the directory, `StateDirectory=easl-site`); and `ufw` allowing only SSH (22/tcp), HTTP (80/tcp), and HTTPS (443/tcp and udp), since the server listens on `127.0.0.1:8000` behind Caddy. Memory use is small (tens of MB, plus ~19 MB per concurrent password hash, at most two at once).

`/etc/easl-site.env` (mode 600, root) holds the email settings, `SMTP_URL` and `MAIL_FROM`; email goes out through Resend (`SMTP_URL=smtps://resend:<API key>@smtp.resend.com:465`, `MAIL_FROM=easl <noreply@easl-lang.org>`), whose DNS records (DKIM, the `send` subdomain, and a `_dmarc` record) are in Cloudflare. `easl-site-mail-test you@example.com` checks them. `systemctl enable --now easl-site` starts the server; `journalctl -u easl-site` shows its logs. It shuts down cleanly on SIGTERM.

Caddy (`deploy/Caddyfile`, installed as `/etc/caddy/Caddyfile`; `systemctl reload caddy` after changing it) serves the docs itself and proxies the editor host. The app sets its own cache headers (a year for `?v=` URLs, revalidation otherwise, `no-store` for pages that depend on the session); the docs keep Caddy's rules. On the docs host, Caddy hands the visit beacon (`/visit`) to the server, and the old playground and the app's paths (`@moved`), which the server answers with its docs-host redirects (it maps `?example=<id>` to the example's sketch, so the mapping lives in one place). Caddy passes the original `Host` through, which is how the server tells the two hosts apart: requests proxied from the `easl-lang.org` block arrive as `Host: easl-lang.org` and get the docs host's redirects. Every name is "DNS only" in Cloudflare (not proxied): the server rate-limits by the client's address, which a proxy would replace with its own, and Caddy gets each name's certificate itself. (The app also compresses with gzip/brotli; Caddy leaves already-encoded responses alone.)

The examples: with `--sync-examples-owner ella`, each start syncs `editor/examples.json` into ella's sketches (and logs and carries on when ella doesn't exist yet). To sync by hand after a site deploy without restarting: `sudo -u easl easl-site-server sync-examples --db /var/lib/easl-site/easl-site.db --site-dir /srv/easl-site`. Ella's account, `ella`, owns them (and is an admin).

**Rolling back the editor** (as root on the machine): `systemctl disable --now easl-site`, then `cp /etc/caddy/Caddyfile.prelaunch /etc/caddy/Caddyfile && systemctl reload caddy`, which serves the docs alone, as before the editor launched.

### The GeoIP database

`deploy/easl-site-geoip` downloads DB-IP's country database (see "Visits") into `/var/lib/easl-site/dbip-country-lite.mmdb`, where the service's `--geoip-db` points, replacing the file only once a whole new one has downloaded; `deploy/easl-site-geoip.timer` runs it on the 5th of each month (DB-IP publishes early in the month; when this month's isn't out, it takes last month's). The server picks the new file up within the hour. Installing it (as root on the server; it needs `curl`):

```sh
install -m 755 deploy/easl-site-geoip /usr/local/sbin/
install -m 644 deploy/easl-site-geoip.service deploy/easl-site-geoip.timer /etc/systemd/system/
systemctl daemon-reload && systemctl enable --now easl-site-geoip.timer
systemctl start easl-site-geoip
```

Until the file exists, the server logs a warning each hour and records visits without countries. `EASL_SITE_GEOIP_DB` overrides where the script puts it.

### Backups

Two layers:

- **Hetzner's backups** of the whole machine (daily, the last 7 kept), turned on in the Hetzner console.
- **A nightly copy of the database**, `deploy/easl-site-backup`, run by `deploy/easl-site-backup.timer` at about 03:30 UTC. The database is a single SQLite file in WAL mode, so copying the file while the server runs can catch it mid-write; the script uses SQLite's online backup instead, turns the copy into one self-contained file, checks its integrity, and keeps the newest 14 in `/var/backups/easl-site/` (root only: the copies hold email addresses and password hashes). Hetzner's backups include those copies. With no database yet it says so and exits successfully. `EASL_SITE_DB`, `EASL_SITE_BACKUP_DIR`, and `EASL_SITE_BACKUP_KEEP` override its paths and count, for trying it out.

Installing it (as root on the server; it needs the `sqlite3` package):

```sh
install -m 755 deploy/easl-site-backup /usr/local/sbin/
install -m 644 deploy/easl-site-backup.service deploy/easl-site-backup.timer /etc/systemd/system/
systemctl daemon-reload && systemctl enable --now easl-site-backup.timer
```

`systemctl start easl-site-backup` runs it now; `journalctl -u easl-site-backup` shows what it did; `systemctl list-timers easl-site-backup.timer` shows the next run.

Restoring: `systemctl stop easl-site`, copy a backup over `/var/lib/easl-site/easl-site.db` (owned by `easl`), delete any `easl-site.db-wal` and `easl-site.db-shm` beside it, and `systemctl start easl-site`.

For a copy off the machine, [Litestream](https://litestream.io) can replicate the database continuously to object storage as a sidecar, with no changes to the app (`litestream replicate /var/lib/easl-site/easl-site.db s3://…`).

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
- **Every page sends a Content-Security-Policy** (`src/pages/mod.rs`): same-origin scripts, styles, and images, no plugins, no framing by other sites. The editor pages' (`EDITOR_CSP`) adds what the editor needs: `'wasm-unsafe-eval'` in `script-src` (WebAssembly, for a browser that applies the page's policy to the analysis worker), `'unsafe-inline'` in `style-src` (Monaco writes `<style>` elements and `style` attributes), and `data:` images (in Monaco's stylesheet); everything it loads, Monaco included, is on its own origin. The iframe a program runs in (`/editor/runner.html`, a static file) sets its own policy in a `<meta>` element (same-origin scripts and styles plus `'wasm-unsafe-eval'` for the runtime and its audio worklet). Static files, workers' scripts included, get no CSP header: a worker started from a same-origin script runs under its own response's policy, so Monaco's worker and the analysis worker run unrestricted.
- **Thumbnails** of non-private sketches are cached publicly for a year under their `?v=` URL; private ones only by the viewer's browser. A sketch made private later can linger in a shared cache under its old URL (no CDN today, so moot for now).
- **Rate limits are in memory**, so they reset on restart, and they assume one server process. The daily email budget is the exception: it's counted in the database, as the backstop behind them.
- **History is budgeted by size, not by count or age** (see "Revision history budget"): what fills a disk is bytes, and counting them lets small programs keep long histories. Sizes are approximate (bytes of paths and contents plus a fixed overhead per row), which is all the budget needs.
- **The head and published revision are never pruned**, even when they alone exceed the sketch's budget (only possible with a budget under 2 MB).
- **Account budgets depend on verification** (5 MB unverified, 50 MB verified) rather than being one figure: what limits how much one person can store is how many accounts they can make, and only verified ones are bounded (by the daily email budget). At 5 sign-ups an hour per address, unverified accounts add at most 25 MB an hour per address, which the free-disk guard backs up.
- **The free-disk guard fails open** when the space can't be measured (logging the error), since refusing every save over a measuring problem would be worse than the risk it guards against.
- **A save past the account budget is refused, not pruned across sketches**: pruning another sketch's history to make room would silently lose versions of work the user isn't looking at.
- **Signing up with the day's email budget spent still creates the account** (unverified, so it can't publish, comment, like, or report until verified), rather than refusing a sign-up for a reason the user can't fix.
- **The sign-up limit counts only sign-ups that could succeed**: mistakes in the form, and taken usernames or emails, are reported before it's checked, so they don't lock out the address.
- **A changed email address's verification email counts against the account's verification limit**, since changing the address repeatedly would otherwise send unlimited email to addresses of the account's choosing.
- **The reset-email limit is per recipient account and silent**: past it, `/forgot` answers as for an unknown address, so a stranger can't flood someone's inbox or learn they have an account.
- **Tags are free-form**, with no curated list or moderation beyond hiding a sketch; the tag row shows whatever listed sketches use most.
- **`seed` tags the examples that have no tags** each time it runs (so an existing database gets them too), and tags the other demo users' sketches when it makes them; on a database seeded before tags existed, those stay untagged.
- **Templates are embedded with `include_str!`** (one list in `src/templates.rs`; a template missing from it fails every test that renders it), and dev mode reads from `CARGO_MANIFEST_DIR`, so `--dev` is meant for `cargo run` in this directory.
- **The origins default from the bind port** (`http://editor.localhost:<port>`, `http://localhost:<port>`), so `--bind 127.0.0.1:8150` works without passing either.
- **The moderation log's `action` and `target_kind` aren't `CHECK`ed in SQL** (unlike `reports.kind`), so a new kind of action needs no table rebuild; `moderation::Action` lists them.
- **An admin deleting a comment counts as moderating only when it's neither their comment nor on their sketch**; as an author or owner they're logged like anyone else (not at all), and the Deleted comments tab labels them "the author" or "the sketch owner".
- **Resolved reports show no actions** on the Reports tab (nothing reopens a report).
- **A user's moderation page** shows the log entries *about* them, not the ones they took as an admin (the Log tab has those).
- **Reserved usernames** are in `src/validate.rs` (`RESERVED_USERNAMES`; `editor` and `playground` are among them)). `ella` isn't reserved; it's Ella's account, which owns the examples.
- **Visits are counted by a beacon from the page**, not from the server's request log: in production Caddy serves the docs from disk, so the server never sees their requests, and counting only browsers that run the page's scripts leaves out most crawlers. Blocking scripts (or the beacon) means not being counted.
- **The visitor salt lives only in memory**, so a restart counts that day's earlier visitors again. Keeping it in the database would survive restarts but put it in the nightly backups, where, with the hashes, it would let addresses be recovered by trying them all.
- **The visitor hash includes the user agent**, so two people behind one address (a household, an office) usually count as two visitors, and one person in two browsers counts as two.
- **Visit rows are kept indefinitely**, since they hold no address and can't be tied to a person after their day.

## Not done

- No email change confirmation to the *old* address, no "sign out everywhere" button, no 2FA.
- No pagination on comments or a profile's playlists.
