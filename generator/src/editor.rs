//! The editor's files: the scripts and styles of the page where visitors
//! write easl sketches and run them (`assets/editor/`), its language
//! support, and the docs' examples as sketches.
//!
//! The editor lives on its own host (`editor.easl-lang.org`), served by the
//! site's server, which renders the page itself (`server/templates/
//! editor.html`) for saved sketches (`/s/<id>`) and new ones (`/new`). It
//! runs each program with the web runtime in a fresh iframe
//! (`assets/editor/runner.html`), so stopping or rerunning a program frees
//! everything it held. The editor is Monaco, loaded from a CDN, with easl's
//! language support compiled to WebAssembly from `editor/analysis` and run
//! in a worker (`analysis-worker.js`): diagnostics, hover,
//! go-to-definition, completion, formatting, and structural editing.
//!
//! The docs' examples (the landing page's program, the files in
//! `pages/editor/`, and every live preview's program) are ordinary sketches
//! there, owned by Ella's account, with ids fixed in
//! `pages/editor/sketch-ids.json` so the docs can link to them. The
//! generator writes them to `editor/examples.json`, which the server's
//! `sync-examples` turns into those sketches, and the build's version and
//! Monaco's to `editor/manifest.json`, for the server.

use std::{
  collections::hash_map::RandomState,
  collections::BTreeMap,
  env, fs,
  hash::{BuildHasher, DefaultHasher, Hash, Hasher},
  path::{Path, PathBuf},
  sync::{Mutex, OnceLock},
};

use easl::{compile_easl_source_to_wgsl, web_bundle::bundle_program};
use serde_json::json;

use crate::{landing::RUNTIME_DIR, nested_build};

/// Where the editor's files go, relative to the site root.
pub const EDITOR_DIR: &str = "editor";

/// The editor's public origin, which the docs link to.
pub const PRODUCTION_EDITOR_ORIGIN: &str = "https://editor.easl-lang.org";

/// The environment variable naming the editor's origin for a build that
/// isn't for production (`./dev.sh` sets it to the local server's).
const EDITOR_ORIGIN_VARIABLE: &str = "EASL_EDITOR_ORIGIN";

/// The analysis crate, relative to the generator's manifest directory.
const ANALYSIS_CRATE_PATH: &str = "../editor/analysis";

/// The web runtime's build output, relative to the generator's manifest
/// directory.
const WEB_RUNTIME_DIST_PATH: &str = "../../easl/web/dist";

/// The runtime build that compiles programs from their sources, which the
/// editor compiles programs with (see `runner.html`).
const COMPILER_RUNTIME_FILES: [&str; 2] =
  ["easl_web_compiler.js", "easl_web_compiler_bg.wasm"];

/// The examples that aren't on any docs page, relative to the generator's
/// manifest directory.
const EXTRA_EXAMPLES_PATH: &str = "../pages/editor";

/// The files in `EXTRA_EXAMPLES_PATH` and their sketches' titles.
const EXTRA_EXAMPLES: &[(&str, &str)] = &[
  ("game-of-life.easl", "Conway's Game of Life"),
  ("wavetable.easl", "Wavetable"),
];

/// Each example's sketch id, by example id, relative to the generator's
/// manifest directory.
const SKETCH_IDS_PATH: &str = "../pages/editor/sketch-ids.json";

/// The program a new sketch starts with.
const NEW_SKETCH: &str = include_str!("../assets/editor/new-sketch.easl");

/// The Monaco editor version the page loads from jsdelivr.
const MONACO_VERSION: &str = "0.52.2";

/// The page's own scripts and styles, copied into `EDITOR_DIR`.
const ASSETS: &[(&str, &str)] = &[
  ("editor.js", include_str!("../assets/editor/editor.js")),
  (
    "documents.js",
    include_str!("../assets/editor/documents.js"),
  ),
  ("api.js", include_str!("../assets/editor/api.js")),
  ("analysis.js", include_str!("../assets/editor/analysis.js")),
  ("features.js", include_str!("../assets/editor/features.js")),
  ("files.js", include_str!("../assets/editor/files.js")),
  ("run.js", include_str!("../assets/editor/run.js")),
  ("social.js", include_str!("../assets/editor/social.js")),
  ("layout.js", include_str!("../assets/editor/layout.js")),
  ("util.js", include_str!("../assets/editor/util.js")),
  ("editor.css", include_str!("../assets/editor/editor.css")),
  (
    "easl-language.js",
    include_str!("../assets/editor/easl-language.js"),
  ),
  (
    "analysis-worker.js",
    include_str!("../assets/editor/analysis-worker.js"),
  ),
  ("new-sketch.easl", NEW_SKETCH),
];

/// One of the docs' example programs, which the editor has as a sketch.
pub struct Example {
  /// Its name: the program's path under `pages/` without `.easl`, `/`
  /// replaced by `-` (`raymarch-materials`), or `landing`.
  pub id: String,
  pub title: String,
  pub description: String,
  pub source: String,
}

/// The editor's origin for this build: `EASL_EDITOR_ORIGIN`, or the
/// production one.
pub fn editor_origin() -> String {
  env::var(EDITOR_ORIGIN_VARIABLE)
    .ok()
    .map(|origin| origin.trim_end_matches('/').to_string())
    .filter(|origin| !origin.is_empty())
    .unwrap_or_else(|| PRODUCTION_EDITOR_ORIGIN.to_string())
}

/// `url` pointing at this build's editor: links the pages write to the
/// production editor (so they work on GitHub too) go to the editor this
/// build is for.
pub fn rewrite_editor_link(url: &str) -> Option<String> {
  let rest = url.strip_prefix(PRODUCTION_EDITOR_ORIGIN)?;
  (rest.is_empty() || rest.starts_with('/') || rest.starts_with('?'))
    .then(|| format!("{}{rest}", editor_origin()))
}

/// The examples' sketch ids, loaded from `SKETCH_IDS_PATH` on first use.
fn sketch_ids() -> &'static Mutex<BTreeMap<String, String>> {
  static IDS: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
  IDS.get_or_init(|| {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(SKETCH_IDS_PATH);
    let ids = fs::read_to_string(&path)
      .ok()
      .map(|text| {
        serde_json::from_str(&text).unwrap_or_else(|e| {
          panic!("{} isn't a JSON object of ids: {e}", path.display())
        })
      })
      .unwrap_or_default();
    Mutex::new(ids)
  })
}

/// The sketch id of the example `example_id`, giving it a new one (saved by
/// `generate`) if it has none yet.
pub fn sketch_id(example_id: &str) -> String {
  let mut ids = sketch_ids().lock().unwrap();
  ids
    .entry(example_id.to_string())
    .or_insert_with(new_sketch_id)
    .clone()
}

/// A random sketch id: 8 base62 characters, like the server's.
fn new_sketch_id() -> String {
  const ALPHABET: &[u8; 62] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let mut bits = RandomState::new().hash_one(std::time::SystemTime::now());
  (0..8)
    .map(|_| {
      let c = ALPHABET[(bits % 62) as usize] as char;
      bits /= 62;
      c
    })
    .collect()
}

/// The editor's address for the example `example_id`'s sketch.
pub fn example_link(example_id: &str) -> String {
  format!("{}/s/{}", editor_origin(), sketch_id(example_id))
}

/// Whether `source` is a whole program the editor can run: it has a `@cpu`
/// entry and compiles on its own.
pub fn is_runnable(source: &str) -> bool {
  source.contains("@cpu")
    && matches!(compile_easl_source_to_wgsl(source), Ok(Ok(_)))
}

/// A link to a new sketch in the editor starting from `source` (carried in
/// the link itself, base64url-encoded).
pub fn code_link(source: &str) -> String {
  format!(
    "{}/new#code={}",
    editor_origin(),
    base64url(source.as_bytes())
  )
}

/// `bytes` in unpadded base64url, which `editor.js` decodes.
fn base64url(bytes: &[u8]) -> String {
  const ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
  let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
  for chunk in bytes.chunks(3) {
    let n = chunk
      .iter()
      .enumerate()
      .fold(0u32, |n, (i, byte)| n | (u32::from(*byte) << (16 - 8 * i)));
    for i in 0..=chunk.len() {
      encoded.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
    }
  }
  encoded
}

/// The programs in `pages/editor/`, as `EXTRA_EXAMPLES` lists them. Each
/// must compile, and every `.easl` file there must be listed.
pub fn extra_examples() -> Vec<Example> {
  let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(EXTRA_EXAMPLES_PATH);
  for entry in fs::read_dir(&dir).unwrap() {
    let path = entry.unwrap().path();
    let name = path.file_name().unwrap().to_string_lossy();
    assert!(
      path.extension().is_none_or(|e| e != "easl")
        || EXTRA_EXAMPLES.iter().any(|(file, _)| *file == name),
      "{} isn't listed in EXTRA_EXAMPLES (editor.rs)",
      path.display()
    );
  }
  EXTRA_EXAMPLES
    .iter()
    .map(|(file, title)| {
      let path = dir.join(file);
      if let Err(errors) = bundle_program(&path) {
        panic!("the example {} doesn't compile:\n{errors}", path.display());
      }
      Example {
        id: path.file_stem().unwrap().to_string_lossy().into_owned(),
        title: title.to_string(),
        description: "An example from the easl docs.".to_string(),
        source: fs::read_to_string(&path).unwrap(),
      }
    })
    .collect()
}

/// Builds the analysis crate to WebAssembly and generates its bindings into
/// `out_dir`.
fn build_analysis(crate_dir: &Path, out_dir: &Path) {
  let status = nested_build::command("cargo")
    .current_dir(crate_dir)
    .args([
      "build",
      "--release",
      "--target",
      "wasm32-unknown-unknown",
      // Compilation recurses deeply on large programs; wasm's default 1MB
      // stack isn't enough (the web runtime sets the same).
      "--config",
      "target.wasm32-unknown-unknown.rustflags=\
       [\"-C\", \"link-arg=-zstack-size=16777216\"]",
    ])
    .status()
    .unwrap_or_else(|e| panic!("couldn't build the editor's analysis: {e}"));
  assert!(
    status.success(),
    "building the editor's analysis failed (see its output above)"
  );
  let wasm = crate_dir
    .join("target/wasm32-unknown-unknown/release/easl_editor_analysis.wasm");
  let status = nested_build::command("wasm-bindgen")
    .args(["--target", "web", "--no-typescript", "--out-dir"])
    .arg(out_dir)
    .arg(&wasm)
    .status()
    .unwrap_or_else(|e| panic!("couldn't run wasm-bindgen: {e}"));
  assert!(
    status.success(),
    "wasm-bindgen failed on the editor's analysis"
  );
}

/// The web runtime's files the editor loads, besides the compiler build
/// (see `runner.html`).
const RUNTIME_FILES: [&str; 4] = [
  "easl_web.js",
  "easl_web_bg.wasm",
  "easl-audio-worklet.js",
  "easl-text-polyfill.js",
];

/// A hash of everything the editor loads: its own files, its analysis
/// module, and the web runtime. The page asks for each file with it as a
/// query (`editor.js?v=<version>`), so a browser never runs a copy it
/// cached from an earlier build, while an unchanged build stays cacheable.
fn build_version(analysis_dir: &Path, runtime_dir: &Path) -> String {
  let mut hasher = DefaultHasher::new();
  for (name, contents) in ASSETS {
    (name, contents).hash(&mut hasher);
  }
  include_str!("../assets/editor/runner.html").hash(&mut hasher);
  let mut analysis_files: Vec<PathBuf> = fs::read_dir(analysis_dir)
    .unwrap()
    .map(|entry| entry.unwrap().path())
    .collect();
  analysis_files.sort();
  let runtime_files = RUNTIME_FILES
    .iter()
    .chain(&COMPILER_RUNTIME_FILES)
    .map(|name| runtime_dir.join(name));
  for path in analysis_files.into_iter().chain(runtime_files) {
    if let Ok(bytes) = fs::read(&path) {
      bytes.hash(&mut hasher);
    }
  }
  format!("{:016x}", hasher.finish())
}

/// Writes the editor's files and `examples` (for the server to keep as
/// sketches), and saves the examples' sketch ids.
pub fn generate(site_dir: &Path, examples: Vec<Example>) {
  let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

  let dir = site_dir.join(EDITOR_DIR);
  fs::create_dir_all(&dir).unwrap();
  build_analysis(
    &manifest_dir.join(ANALYSIS_CRATE_PATH),
    &dir.join("analysis"),
  );
  let runtime_dir = site_dir.join(RUNTIME_DIR);
  let dist = manifest_dir.join(WEB_RUNTIME_DIST_PATH);
  for name in COMPILER_RUNTIME_FILES {
    fs::copy(dist.join(name), runtime_dir.join(name)).unwrap_or_else(|e| {
      panic!(
        "couldn't copy the web runtime's compiler build ({}): {e}",
        dist.join(name).display()
      )
    });
  }
  let version = build_version(&dir.join("analysis"), &runtime_dir);
  for (name, contents) in ASSETS {
    fs::write(dir.join(name), contents.replace("__VERSION__", &version))
      .unwrap();
  }
  fs::write(
    dir.join("runner.html"),
    include_str!("../assets/editor/runner.html")
      .replace("__RUNTIME__", &format!("../{RUNTIME_DIR}"))
      .replace("__VERSION__", &version),
  )
  .unwrap();

  assert!(
    is_runnable(NEW_SKETCH),
    "the new sketch program (assets/editor/new-sketch.easl) doesn't compile"
  );
  let examples_json = json!({
    "examples": examples
      .iter()
      .map(|example| json!({
        "id": example.id,
        "sketch_id": sketch_id(&example.id),
        "title": example.title,
        "description": example.description,
        "files": [{ "path": "main.easl", "content": example.source }],
      }))
      .collect::<Vec<_>>(),
  })
  .to_string();
  fs::write(dir.join("examples.json"), examples_json).unwrap();
  fs::write(
    dir.join("manifest.json"),
    json!({ "version": version, "monaco_version": MONACO_VERSION }).to_string(),
  )
  .unwrap();
  save_sketch_ids(&manifest_dir.join(SKETCH_IDS_PATH));
}

/// Writes the examples' sketch ids back, if any example got a new one.
fn save_sketch_ids(path: &Path) {
  let ids = sketch_ids().lock().unwrap();
  let text = format!("{}\n", serde_json::to_string_pretty(&*ids).unwrap());
  if fs::read_to_string(path).ok().as_deref() != Some(text.as_str()) {
    fs::write(path, text).unwrap();
  }
}
