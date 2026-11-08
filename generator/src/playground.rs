//! The playground (`playground.html`): an editor where visitors write easl
//! and run it on a canvas beside it.
//!
//! The page runs each program with the web runtime in a fresh iframe
//! (`assets/playground/runner.html`), so stopping or rerunning a program
//! frees everything it held. The editor is Monaco, loaded from a CDN, with
//! easl's language support compiled to WebAssembly from
//! `playground/analysis` and run in a worker (`analysis-worker.js`):
//! diagnostics, hover, go-to-definition, completion, formatting, and
//! structural editing.
//!
//! The examples it offers are the landing page's program, the files in
//! `pages/playground/`, and every live preview's program.

use std::{
  fs,
  hash::{DefaultHasher, Hash, Hasher},
  path::{Path, PathBuf},
};

use easl::{compile_easl_source_to_wgsl, web_bundle::bundle_program};
use serde_json::json;

use crate::{landing::RUNTIME_DIR, nested_build};

/// Where the playground's files go, relative to the site root.
pub const PLAYGROUND_DIR: &str = "playground";

/// The output page, relative to the site root.
pub const PLAYGROUND_PAGE: &str = "playground.html";

/// The analysis crate, relative to the generator's manifest directory.
const ANALYSIS_CRATE_PATH: &str = "../playground/analysis";

/// The web runtime's build output, relative to the generator's manifest
/// directory.
const WEB_RUNTIME_DIST_PATH: &str = "../../easl/web/dist";

/// The runtime build that compiles programs from their sources, which the
/// playground runs programs with. A web runtime from before the runtime was
/// split in two doesn't have it; its one build compiles programs itself, and
/// the playground uses that instead (see `runner.html`).
const COMPILER_RUNTIME_FILES: [&str; 2] =
  ["easl_web_compiler.js", "easl_web_compiler_bg.wasm"];

/// Extra example programs, relative to the generator's manifest directory.
const EXTRA_EXAMPLES_PATH: &str = "../pages/playground";

/// The files in `EXTRA_EXAMPLES_PATH` and their titles, in menu order.
const EXTRA_EXAMPLES: &[(&str, &str)] = &[
  ("game-of-life.easl", "Conway's Game of Life"),
  ("theremin.easl", "Theremin: sound from the mouse"),
];

/// The program a new sketch starts with.
const NEW_SKETCH: &str = include_str!("../assets/playground/new-sketch.easl");

/// The Monaco editor version the page loads from jsdelivr.
const MONACO_VERSION: &str = "0.52.2";

/// The page's own scripts and styles, copied into `PLAYGROUND_DIR`.
const ASSETS: &[(&str, &str)] = &[
  (
    "playground.js",
    include_str!("../assets/playground/playground.js"),
  ),
  (
    "playground.css",
    include_str!("../assets/playground/playground.css"),
  ),
  (
    "easl-language.js",
    include_str!("../assets/playground/easl-language.js"),
  ),
  (
    "analysis-worker.js",
    include_str!("../assets/playground/analysis-worker.js"),
  ),
];

/// A program the playground offers to start from.
pub struct Example {
  /// Its name in the page's URL (`playground.html?example=<id>`).
  pub id: String,
  /// The submenu it's listed in, or `None` for the menu's top level.
  pub menu: Option<String>,
  pub title: String,
  pub source: String,
}

/// A link to the playground showing `example`, from a page `prefix` away
/// from the site root.
pub fn link(prefix: &str, example: &str) -> String {
  format!("{prefix}{PLAYGROUND_PAGE}?example={example}")
}

/// Whether `source` is a whole program the playground can run: it has a
/// `@cpu` entry and compiles on its own.
pub fn is_runnable(source: &str) -> bool {
  source.contains("@cpu")
    && matches!(compile_easl_source_to_wgsl(source), Ok(Ok(_)))
}

/// A link to the playground opening `source` (carried in the link itself,
/// base64url-encoded), from a page `prefix` away from the site root.
pub fn code_link(prefix: &str, source: &str) -> String {
  format!(
    "{prefix}{PLAYGROUND_PAGE}#code={}",
    base64url(source.as_bytes())
  )
}

/// `bytes` in unpadded base64url, which `playground.js` decodes.
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

/// The programs in `pages/playground/`, as `EXTRA_EXAMPLES` lists them.
/// Each must compile, and every `.easl` file there must be listed.
pub fn extra_examples() -> Vec<Example> {
  let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(EXTRA_EXAMPLES_PATH);
  for entry in fs::read_dir(&dir).unwrap() {
    let path = entry.unwrap().path();
    let name = path.file_name().unwrap().to_string_lossy();
    assert!(
      path.extension().is_none_or(|e| e != "easl")
        || EXTRA_EXAMPLES.iter().any(|(file, _)| *file == name),
      "{} isn't listed in EXTRA_EXAMPLES (playground.rs)",
      path.display()
    );
  }
  EXTRA_EXAMPLES
    .iter()
    .map(|(file, title)| {
      let path = dir.join(file);
      if let Err(errors) = bundle_program(&path) {
        panic!(
          "the playground example {} doesn't compile:\n{errors}",
          path.display()
        );
      }
      Example {
        id: path.file_stem().unwrap().to_string_lossy().into_owned(),
        menu: None,
        title: title.to_string(),
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
    .unwrap_or_else(|e| panic!("couldn't build the playground analysis: {e}"));
  assert!(
    status.success(),
    "building the playground analysis failed (see its output above)"
  );
  let wasm = crate_dir.join(
    "target/wasm32-unknown-unknown/release/easl_playground_analysis.wasm",
  );
  let status = nested_build::command("wasm-bindgen")
    .args(["--target", "web", "--no-typescript", "--out-dir"])
    .arg(out_dir)
    .arg(&wasm)
    .status()
    .unwrap_or_else(|e| panic!("couldn't run wasm-bindgen: {e}"));
  assert!(
    status.success(),
    "wasm-bindgen failed on the playground analysis"
  );
}

/// The web runtime's files the playground loads, besides the compiler build
/// (see `runner.html`).
const RUNTIME_FILES: [&str; 4] = [
  "easl_web.js",
  "easl_web_bg.wasm",
  "easl-audio-worklet.js",
  "easl-text-polyfill.js",
];

/// A hash of everything the playground loads: its own files, its analysis
/// module, and the web runtime. The page asks for each file with it as a
/// query (`playground.js?v=<version>`), so a browser never runs a copy it
/// cached from an earlier build, while an unchanged build stays cacheable.
fn build_version(analysis_dir: &Path, runtime_dir: &Path) -> String {
  let mut hasher = DefaultHasher::new();
  for (name, contents) in ASSETS {
    (name, contents).hash(&mut hasher);
  }
  include_str!("../assets/playground/runner.html").hash(&mut hasher);
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

/// Writes the playground page and its files, offering `examples` in its
/// menu, with `default_example` first shown.
pub fn generate(
  site_dir: &Path,
  examples: Vec<Example>,
  default_example: &str,
) {
  let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

  let dir = site_dir.join(PLAYGROUND_DIR);
  fs::create_dir_all(&dir).unwrap();
  build_analysis(
    &manifest_dir.join(ANALYSIS_CRATE_PATH),
    &dir.join("analysis"),
  );
  let runtime_dir = site_dir.join(RUNTIME_DIR);
  let dist = manifest_dir.join(WEB_RUNTIME_DIST_PATH);
  let has_compiler_build = COMPILER_RUNTIME_FILES
    .iter()
    .all(|name| dist.join(name).exists());
  if has_compiler_build {
    for name in COMPILER_RUNTIME_FILES {
      fs::copy(dist.join(name), runtime_dir.join(name)).unwrap();
    }
  }
  let version = build_version(&dir.join("analysis"), &runtime_dir);
  for (name, contents) in ASSETS {
    fs::write(dir.join(name), contents.replace("__VERSION__", &version))
      .unwrap();
  }
  fs::write(
    dir.join("runner.html"),
    include_str!("../assets/playground/runner.html")
      .replace("__RUNTIME__", &format!("../{RUNTIME_DIR}"))
      .replace("__HAS_COMPILER_BUILD__", &has_compiler_build.to_string())
      .replace("__VERSION__", &version),
  )
  .unwrap();

  assert!(
    is_runnable(NEW_SKETCH),
    "the new sketch program (assets/playground/new-sketch.easl) doesn't \
     compile"
  );
  let examples_json = json!({
    "default": default_example,
    "newSketch": NEW_SKETCH,
    "examples": examples
      .iter()
      .map(|example| json!({
        "id": example.id,
        "menu": example.menu,
        "title": example.title,
        "source": example.source,
      }))
      .collect::<Vec<_>>(),
  })
  .to_string()
  // The JSON sits in a <script> element, which a literal "</" could end.
  .replace("</", "<\\/");
  fs::write(
    site_dir.join(PLAYGROUND_PAGE),
    include_str!("../assets/playground/playground.html")
      .replace("__MONACO_VERSION__", MONACO_VERSION)
      .replace("__PLAYGROUND__", PLAYGROUND_DIR)
      .replace("__VERSION__", &version)
      .replace("__EXAMPLES__", &examples_json),
  )
  .unwrap();
}
