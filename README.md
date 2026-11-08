# Easl documentation & site

The documentation for [easl](https://github.com/Ella-Hoeppner/easl) lives as plain Markdown in [`pages/`](pages/), readable directly on GitHub or in any Markdown viewer. A static site generator in [`generator/`](generator/) turns those pages into a styled HTML site, and also builds the **landing page**, a single screen with a live, WebGPU-rendered easl program as its background, and the **playground**.

## Sibling checkouts

Building the site needs two other repositories checked out next to this one:

- [`easl`](https://github.com/Ella-Hoeppner/easl) at `../easl`: the compiler (a path dependency of the generator and the playground's analysis crate) and its web runtime (`../easl/web`), which the build compiles to WebAssembly.
- [`easl_lsp`](https://github.com/Ella-Hoeppner/easl_lsp) at `../easl_lsp`: the playground's language support compiles some of its source files directly (see "The playground").

The pages document the easl in that `../easl` checkout, and every program in them must compile with it, or the build fails.

## Building the site

```
cd generator
cargo run
```

This writes the site to `site/`. Serve it with any static file server (e.g. `python3 -m http.server` in `site/`): the landing page loads the web runtime as an ES module and a `.wasm` file, which browsers won't do from `file://`. The hero animation needs a WebGPU-capable browser (current Chrome/Edge/Safari) and degrades to a static gradient elsewhere.

The build also builds the easl web runtime (`../easl/web/build.sh`), so it needs the `wasm32-unknown-unknown` target and `wasm-bindgen-cli` at the version pinned in easl's `web/Cargo.toml` (see `web/build.sh` there).

For a live preview while editing, rebuild on every change (refresh the browser to see it):

```
cd generator
cargo watch -w ../pages -w assets -w src -x run
```

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

## The playground

`playground.html` is an editor where visitors write easl and run it, built by `generator/src/playground.rs` from `generator/assets/playground/`.

- **Examples:** the landing page's program, the `.easl` files in `pages/playground/` (listed with their titles in `EXTRA_EXAMPLES` in `playground.rs`; the build fails on an unlisted one), and every live preview's program (linked from the preview as "Open in the playground"). A page's live previews are listed in one submenu, as Part 1, Part 2, …; the submenu is named by the page's `playground_menu` in `generator/src/main.rs`, or else its sidebar title. Each must compile, or the build fails. `playground.html?example=<id>` opens one; the id is the program's path under `pages/` without `.easl`, with `/` replaced by `-` (`raymarch-materials`), and the landing program's is `landing`.
- **The editor** is [Monaco](https://microsoft.github.io/monaco-editor/), loaded from jsdelivr at a pinned version (`MONACO_VERSION` in `playground.rs`). `easl-language.js` gives it easl's colors, bracket and quote auto-closing, and the theme. Enter indents by the forms around the cursor.
- **Language support** is [easl_lsp](https://github.com/Ella-Hoeppner/easl_lsp)'s, compiled to WebAssembly by `playground/analysis/` and run in a worker (`analysis-worker.js`): diagnostics, hover (types and declarations), go-to-definition, completion, formatting, semantic tokens, and structural editing. The crate includes easl_lsp's `source.rs`, `syntax.rs`, `completion.rs`, and `structural.rs` unchanged from the sibling `../easl_lsp` checkout, so building the site needs that repository next to this one. `analysis.rs` is easl_lsp's analysis cut down to a single document, since the original reads imported files from disk; keep the two in step. The generator builds the crate on every run (it needs the wasm32 target and `wasm-bindgen`, like the web runtime).
- **Running:** each run gets a fresh iframe (`runner.html`) running the program with the web runtime in `site/easl-web/`, so stopping or rerunning frees everything the program held. A program only runs once the analysis has finished without errors. The analysis worker is restarted if it takes more than 10 seconds or crashes, so a program that would hang or crash the compiler never reaches the runtime, which compiles on the page's own thread.
- **Code blocks:** an `easl` code block on a docs page that's a whole program (it has a `@cpu` entry and compiles on its own) gets an "Open in the playground" link, which carries the program in the URL (`playground.html#code=…`). A block fenced as ` ```easl no-playground ` doesn't, for programs the playground can't run (file I/O) or that a live preview already links.
- **Saving:** each example's edits are kept in the browser's localStorage until reset.
- **Sketches:** "New sketch…" at the bottom of the example menu makes a program of the visitor's own, starting from `generator/assets/playground/new-sketch.easl` ("Sketch 1", …), listed below a line in the menu. Sketches live in localStorage (the list as `sketches`, each one's text as `sketch:<id>`), can be renamed, and have no Reset.

## Layout

- `pages/guide.md`, `pages/language.md`, `pages/shaders.md`, `pages/cpu.md` — the guide pages
- `pages/raymarch.md` — the ray marching walkthrough, and `pages/raymarch/` the programs its live previews run
- `pages/landing/`, `pages/main_page_example.easl` — the landing page's text and program
- `pages/playground/` — the playground's own examples; `playground/analysis/` — its language support, compiled to WebAssembly
- `pages/reference/*.md` — the builtin function reference, one page per category
- `generator/` — the site generator: parses the Markdown with pulldown-cmark, applies a hand-rolled easl syntax highlighter to ```` ```easl ```` code fences, and wraps everything in a template with sidebar navigation and per-page tables of contents; `generator/src/landing.rs` builds the landing page
- `generator/assets/` — the docs stylesheet, the landing page's template and stylesheet, and the playground's page, scripts, and styles (`assets/playground/`)
- `site/` — the generated output (disposable; regenerate any time); `site/index.html` is the landing, `site/guide.html` is the docs entry point, `site/easl-web/` the hero program and the web runtime

## Conventions the generator relies on

- The nav order and page titles are declared in the `PAGES` table at the top of `generator/src/main.rs`; add new pages there.
- Every `###` heading on a reference page (other than `reference/overview.md`) is treated as a function entry: each backticked name in the heading becomes an entry in the generated, filterable **function index** page, linking to that heading.
- When a heading abbreviates a family of names (e.g. `` `vec2f`…`vec4b` ``), an HTML comment of the form `<!-- index: vec2f vec3f ... -->` placed after the heading adds the elided names to the function index.
- Relative links between `.md` files are rewritten to `.html` in the generated site, so cross-page links work both on GitHub and in the site.
- Heading anchors use GitHub's slug style, so `#section-name` fragment links also work in both places. Operator headings (whose names are all symbols) get readable generated anchors like `#op-plus-eq`.
