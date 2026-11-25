# easl_site

The easl docs (`easl-lang.org`, static) and the online editor (`editor.easl-lang.org`, a Rust server). `README.md` covers the layout, building, and running; `server/README.md` covers the server in depth (routes, API, limits, moderation, production, backups, decisions). Read the relevant one before changing anything.

## Layout

- `pages/`: the docs, as Markdown. `generator/` (Rust) builds them, the landing page, and the editor's assets into `site/` (gitignored). The generator builds against pinned easl releases: the `easl` crate from crates.io, and the web runtime and easl_lsp's modules from pinned GitHub revisions (`generator/src/sources.rs`). A gitignored `.cargo/config.toml` (copied from `.cargo/local-easl.toml`) switches builds to the local `../easl` and `../easl_lsp` checkouts; see README.md, "The easl it builds with".
- `generator/assets/editor/`: the editor's JavaScript and CSS; `editor/analysis/`: its language support, compiled to WebAssembly. Monaco is an npm dependency (`generator/package.json`), copied into the site at build time.
- `server/`: the editor's server (axum, sqlx with SQLite, minijinja). Templates in `server/templates/` (the rules and privacy page's prose is `rules.md` there), static assets in `server/static/`, migrations in `server/migrations/`, deployment files in `server/deploy/`.

## Working here

- `PORT=8000 ./dev.sh` builds everything and runs the site locally: docs at http://localhost:8000, editor at http://editor.localhost:8000 (users ella, alice, bob, carol; password `password123`). Ella usually has it running on port 8000: don't stop it unless you're restarting it for her, and test your own changes on another port with a scratch database.
- Stop only processes you started, by PID. Never `pkill`, `killall`, or pattern-matched kills: they take down Ella's server and anything else running.
- Don't modify `server/data/easl-site.db` (Ella's local demo data) except through the running site; back it up before a migration runs on it.
- Server checks: `cd server && cargo fmt && cargo clippy --all-targets && cargo test`. Run `cargo fmt` on Rust you touch (Ella formats on save, and stale formatting pollutes her diffs). The generator has pre-existing formatting differences; don't reformat files you didn't change.
- Comments describe current behavior, not history. Imports go at the file header.
- The editor is a self-contained site: the docs link to it only from the sidebar's Editor link, and nothing about accounts, rules, or privacy appears on the docs side.
- Claude owns the implementation here: commit your work without asking, once fmt, clippy, and tests pass, and skip review guides and the diff viewer. Ella writes the hand-edited Markdown (`pages/*.md`, `server/templates/rules.md`): tell her whenever you change one, and leave her uncommitted edits to them out of your commits.
- Don't use `git stash` or `git checkout` in this checkout (Ella edits concurrently).
- Never deploy (`./deploy.sh`) or touch the production server (`ssh easl-site`) without Ella asking.
