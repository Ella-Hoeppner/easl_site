# Easl documentation & site

The documentation for [easl](https://github.com/Ella-Hoeppner/easl) lives as plain Markdown in [`pages/`](pages/), readable directly on GitHub or in any Markdown viewer. A static site generator in [`generator/`](generator/) turns those pages into a styled HTML site, and also builds the **landing page**, a single screen with a live, WebGPU-rendered easl program as its background. The **editor** (at `editor.easl-lang.org`), where people write, save, and share easl sketches, is in the same repository: its page and language support in `generator/` and `editor/`, and the server behind it in `server/`.

## The easl it builds with

The site builds with released, pinned versions of easl, so it needs nothing else checked out:

- **The compiler** is the [`easl`](https://crates.io/crates/easl) crate from crates.io, a dependency of the generator and the editor's analysis crate (`easl = "0.2.0"` in both manifests).
- **The web runtime** (the `web/` crate in [easl](https://github.com/Ella-Hoeppner/easl)'s repository), which the build compiles to WebAssembly, comes from the tag of that same release, `v0.2.0`.
- **easl_lsp's modules**, which the editor's language support compiles in (see "The editor"), come from a pinned commit of [easl_lsp](https://github.com/Ella-Hoeppner/easl_lsp).

The generator fetches the last two from GitHub into `generator/target/sources/` the first time it needs them (`generator/src/sources.rs` holds the tag and the commit). The pages document that release of easl, and every program in them must compile with it, or the build fails.

**Updating easl:** after publishing a release, change the version in `generator/Cargo.toml` and `editor/analysis/Cargo.toml` and the tag in `sources.rs` together (the build fails when they disagree), and the easl_lsp commit too when its modules changed. Pick an easl_lsp commit that's pushed and builds against that release.

### Building against local checkouts

To preview unreleased easl or easl_lsp changes, build against your checkouts at `../easl` and `../easl_lsp` instead:

```
cp .cargo/local-easl.toml .cargo/config.toml    # on
rm .cargo/config.toml                           # off
```

While it's on (it's gitignored), Cargo patches the `easl` crate to `../easl` and the generator builds the runtime and easl_lsp's modules from the checkouts (`EASL_DIR`, `EASL_LSP_DIR`), saying so at the start of the build. The generator's and the analysis crate's `Cargo.lock` then point at the local crate; turn it off and build again before committing them. `deploy.sh` won't deploy the site while it's on.

## Running the site

The site has two hosts: the docs (`easl-lang.org`, static, built by `generator/` into `site/`) and the editor (`editor.easl-lang.org`), served by `server/` (see its README): accounts, the editor's pages, saved sketches, explore, profiles, comments, and playlists. To run it all locally:

```
./dev.sh            # or PORT=8100 ./dev.sh
```

That builds the site, seeds a demo database the first time (`server/data/easl-site.db`; sign in as `ella`, `alice`, `bob`, or `carol` with the password `password123`), and starts the server in development mode, with the docs at http://localhost:8000 and the editor at http://editor.localhost:8000 (Chrome and Firefox send every `*.localhost` name to this machine; for Safari, add `127.0.0.1 editor.localhost` to `/etc/hosts`). Development mode shows email links (verification, password reset) on the page instead of sending them. Delete `server/data/` to start over.

The docs link to the editor at `https://editor.easl-lang.org` unless `EASL_EDITOR_ORIGIN` names another (`dev.sh` sets it to the local one). Pages can write links to the editor with its production address (`https://editor.easl-lang.org/s/<id>`), which works on GitHub too; the generator rewrites them for the editor it's building for.

The build also builds the easl web runtime (easl's `web/build.sh`), so it needs `git`, the `wasm32-unknown-unknown` target, and `wasm-bindgen-cli` at the version pinned in easl's `web/Cargo.toml` (see `web/build.sh` there). It also needs Node.js with npm, which installs the editor's JavaScript libraries (see "The editor"); the build runs `npm ci` itself when they're missing or out of date.

To rebuild the site on every change to the pages or the editor while the server runs (refresh the browser to see it; the server picks up a rebuilt site without restarting):

```
cd generator
EASL_EDITOR_ORIGIN=http://editor.localhost:8000 cargo watch -w ../pages -w assets -w src -x run
```

The server reloads its own templates (`server/templates/`) and page assets (`server/static/`) on every request in development mode.

## Deploying

This describes the static site as it's deployed now. With the server, production becomes the server binary behind Caddy (see `server/README.md` for the systemd unit, the Caddy config, and backups); `deploy.sh` doesn't do that yet.

`./deploy.sh` builds the site from scratch and uploads it to the server behind easl-lang.org, replacing what's there. It connects through an SSH host named `easl-site`, which `~/.ssh/config` defines:

```
Host easl-site
  HostName 2.31.116.198
  User root
  IdentityFile ~/.ssh/easl_site
  IdentitiesOnly yes
```

The server (a Hetzner Cloud machine running Ubuntu) serves `/srv/easl-site` with [Caddy](https://caddyserver.com), which gets the HTTPS certificates itself (WebGPU only runs on HTTPS pages) and compresses responses. Its configuration is `/etc/caddy/Caddyfile`: `www.easl-lang.org` redirects to `easl-lang.org`, and every file is served `Cache-Control: no-cache` (browsers revalidate it on each load, so a deploy shows up at once) except the editor's, which it asks for with its build's hash (`?v=…`) and which are kept for good. Uploads go in as the `deploy` user, which owns `/srv/easl-site`; root logs in with the same key, for administering the server. The domain's DNS is at Cloudflare, with the proxy off ("DNS only"), so that Caddy can get certificates for it.

## The landing page

Everything on the landing page (`generator/src/landing.rs`) comes from `pages/`, read on every build, so editing it means editing those files:

- `pages/landing/*.md` holds all of its text: `nav.md` (the links at the top), `hero.md`, `getting-started.md`, `program-note.md` (the caption under the code), and `no-webgpu.md` (the caption shown instead when the browser has no WebGPU).
- `hero.md` has to start with the name as a heading, then the tagline, then a description. Those three also make the page's `<title>` and meta description. Every link in it renders as a button.
- `pages/main_page_example.easl` is the program. The same file is both displayed on the page and run as its background, with the easl web runtime: the build validates it (and naga-validates its WGSL), then packages it the way `easl compile --web` does into `site/easl-web/`. If it stops compiling, the site stops building. The page's canvas is the program's window, so `window-resolution` is the canvas's size in physical pixels.

The page's markup is `generator/assets/landing.html`, a `format!` template, so literal braces in it are doubled. The code panel's text scales with the window so that every line fits on screen, down to a readable minimum, and the panel's transparency is set by `--code-panel-opacity` and `--code-panel-opacity-hover` at the top of `generator/assets/landing.css`.

## Live previews

A page can run an easl program on a canvas inline with an HTML comment, which GitHub and other Markdown viewers hide:

```
<!-- live-preview: raymarch/sphere.easl -->
<!-- live-preview: raymarch/cpu-raymarch.easl console -->
```

The path is relative to the page's own Markdown file, and `console` adds a box under the canvas showing the program's `print` output. The build fails if a program doesn't compile, and packages each one into `site/easl-web/` the way `easl compile --web` does; `generator/assets/live-previews.js` starts each preview the first time it nears the screen, so a page's previews don't all start at once, and pauses it whenever it's off screen.

The ray marching walkthrough's programs live in `pages/raymarch/`, each one the walkthrough's program as it stands at its preview. They're separate copies of the code shown in the page, so a change to the walkthrough's code blocks needs the matching change there.

## The editor

The editor (`editor.easl-lang.org`) is where people write easl sketches and run them: new ones at `/new`, saved ones at `/s/<id>`. The site's server (`server/`, see its README) renders the page from `server/templates/editor.html`; its scripts and styles are in `generator/assets/editor/`, which the generator copies into `site/editor/` with a content hash in their URLs (`?v=…`), writing the hash to `site/editor/manifest.json` for the server.

- **Documents** (`documents.js`): the page edits one sketch: a saved one (its owner's edits kept in this browser as a draft until saved; anyone else can edit it here and fork it), or a new one (a draft until it's saved to the visitor's account).
- **Files** (`files.js`): a sketch has one or more files, in folders if it likes. The file explorer beside the editor (collapsible) lists them and makes, renames, moves (by renaming), and deletes files and folders; the tabs above the editor are the files that are open. One file imports another with `(import "lib/noise.easl")`, relative to its own folder (`..` works), and completion offers the other files' paths inside an import. Folders without files are kept only in drafts.
- **Running** (`run.js`): the bar above the canvas has the run, pause, and stop buttons, and the sketch's entry points: every `@cpu` function in any of its files (as buttons, or a dropdown when there are more than three). Each also has a ▶ in the editor's gutter and a "Run" link in its `@cpu` annotation's hover. Running one makes it the sketch's entry (saved with it). Each run gets a fresh iframe (`runner.html`) running the program with the web runtime in `site/easl-web/`, so stopping or rerunning frees everything the program held. A version of a sketch's files is compiled once, with the entry's file as the main file, by the runtime's compiler build (easl's `compileProgram`, which keeps every `@cpu` entry point and the source map's descriptions, so runtime errors still say where they happened); its bytes are kept (the last 8 versions), and every run of that version, at any entry point, starts from them (`startProgram` with `host.entry`) on the much smaller plain build. A program only runs once the check of its entry's file has finished without errors; the analysis worker is restarted if it takes more than 10 seconds or crashes, so a program that would hang or crash the compiler never reaches the runtime, which compiles on the page's own thread. The runner also takes the pictures that become sketches' thumbnails, at the moment the program submits a frame (a WebGPU canvas can't be read after it's presented).
- **Drafts and publishing:** every save to the server is kept as a revision. While a sketch is unlisted or public, its page shows its published revision, and what its owner saves is a draft: the bar marks it (DRAFT), with **Publish** (the draft becomes what everyone sees) and **View published** (the published version, read-only and runnable, with **Back to your draft** and **Discard draft**). A private sketch is just its latest save. Making a sketch unlisted or public publishes what's saved. See `server/README.md`, "Drafts and publishing".
- **The bar** shows the sketch's title (click to rename one of yours), its author (linking to their profile), Like, and ＋ Playlist (add it to one of your playlists, or start a new one), then Format, Save (or Fork, for someone else's), Revert, and Share. The tabs under the canvas hold the console, the comments, and About (description, visibility, forking, reporting, deleting). The editor talks to the server through `api.js`.
- **The editor** is [Monaco](https://microsoft.github.io/monaco-editor/), an npm dependency pinned in `generator/package.json` and `generator/package-lock.json`. The generator installs it into `generator/node_modules/` (running `npm ci` there when that doesn't match the lockfile) and copies the parts of its AMD build the page loads (`min/vs/`'s loader, editor, and worker, not the other languages) to `site/editor/monaco/<version>/vs/`, writing the version to `site/editor/manifest.json`, from which the server's page loads it. Everything the editor loads comes from its own origin. `easl-language.js` gives it easl's colors, bracket and quote auto-closing, and the theme. Enter indents by the forms around the cursor.
- **Updating Monaco:** change its version in `generator/package.json`, run `npm install` in `generator/` (which updates `package-lock.json`; commit both), and rebuild the site. A build whose `package.json` and lockfile disagree stops and says to run `npm install`. A new version's files go to a new path, so browsers never mix cached files of two versions. Within 0.52 that's all; Monaco 0.53 rearranged its `min/vs/` build (bundled chunks with hashed names, no `vs/base/worker/workerMain.js`), so moving past 0.52 means updating which parts the generator copies (`MONACO_FILES` in `editor.rs`, whose build stops naming any that's missing) and how `editor.js` loads it and starts its worker (`loadMonaco`).
- **Language support** (`analysis.js`, `features.js`) is [easl_lsp](https://github.com/Ella-Hoeppner/easl_lsp)'s, compiled to WebAssembly by `editor/analysis/` and run in a worker (`analysis-worker.js`): diagnostics, hover (types and declarations), go-to-definition (into the other files too), completion, formatting, semantic tokens, and structural editing. The file being edited is checked as the main file of its own program, with the sketch's other files available to import, and errors in a file it imports appear on the import. The crate includes easl_lsp's `source.rs`, `syntax.rs`, `completion.rs`, and `structural.rs` unchanged, through its `easl_lsp` link (gitignored), which the generator points at the easl_lsp checkout it builds from (see "The easl it builds with"). `analysis.rs` is easl_lsp's analysis with the sketch's files in place of the filesystem; keep the two in step. The generator builds the crate on every run (it needs the wasm32 target and `wasm-bindgen`, like the web runtime).
- **The docs' examples** (the landing page's program, the `.easl` files in `pages/editor/`, listed with their titles in `EXTRA_EXAMPLES` in `editor.rs`, and every live preview's program) are ordinary sketches in the editor, owned by Ella's account (`ella`). Each has a sketch id fixed in `pages/editor/sketch-ids.json` (the generator gives a new example one and saves it), so the docs link to `/s/<id>`; the generator writes the examples to `site/editor/examples.json`, and the server's `sync-examples` (run at startup with `--sync-examples-owner ella`) keeps those sketches in step. A new sketch starts from `generator/assets/editor/new-sketch.easl`.
- **Code blocks:** an `easl` code block on a docs page that's a whole program (it has a `@cpu` entry and compiles on its own) gets an "Open in the editor" link, which opens a new sketch with the program carried in the URL (`/new#code=…`). A block fenced as ` ```easl no-editor ` doesn't, for programs the editor can't run (file I/O) or that a live preview already links.

## Layout

- `pages/guide.md`, `pages/language.md`, `pages/shaders.md`, `pages/cpu.md` — the guide pages
- `pages/raymarch.md` — the ray marching walkthrough, and `pages/raymarch/` the programs its live previews run
- `pages/landing/`, `pages/main_page_example.easl` — the landing page's text and program
- `pages/editor/` — the editor's own examples and the examples' sketch ids; `editor/analysis/` — the editor's language support, compiled to WebAssembly
- `pages/reference/*.md` — the builtin function reference, one page per category
- `generator/` — the site generator: parses the Markdown with pulldown-cmark, applies a hand-rolled easl syntax highlighter to ```` ```easl ```` code fences, and wraps everything in a template with sidebar navigation and per-page tables of contents; `generator/src/landing.rs` builds the landing page
- `generator/assets/` — the docs stylesheet, the landing page's template and stylesheet, and the editor's scripts and styles (`assets/editor/`)
- `site/` — the generated output (disposable; regenerate any time); `site/index.html` is the landing, `site/guide.html` is the docs entry point, `site/easl-web/` the hero program and the web runtime

## Conventions the generator relies on

- The nav order and page titles are declared in the `PAGES` table at the top of `generator/src/main.rs`; add new pages there.
- Every `###` heading on a reference page (other than `reference/overview.md`) is treated as a function entry: each backticked name in the heading becomes an entry in the generated, filterable **function index** page, linking to that heading.
- When a heading abbreviates a family of names (e.g. `` `vec2f`…`vec4b` ``), an HTML comment of the form `<!-- index: vec2f vec3f ... -->` placed after the heading adds the elided names to the function index.
- Relative links between `.md` files are rewritten to `.html` in the generated site, so cross-page links work both on GitHub and in the site.
- Heading anchors use GitHub's slug style, so `#section-name` fragment links also work in both places. Operator headings (whose names are all symbols) get readable generated anchors like `#op-plus-eq`.
