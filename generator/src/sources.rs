//! The easl sources the site builds besides the `easl` crate (which comes
//! from crates.io): the web runtime (`web/` in the easl repository) and the
//! easl_lsp modules the editor's analysis crate compiles in.
//!
//! By default they're pinned revisions, fetched from GitHub into
//! `target/sources/` the first time a build needs them: the easl release
//! whose version the manifests name, and an easl_lsp commit. Setting
//! `EASL_DIR` and `EASL_LSP_DIR` builds from local checkouts instead, which
//! `.cargo/local-easl.toml` does along with patching the `easl` crate (see
//! the README, "Building against local checkouts").

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The easl repository, and the tag the web runtime is built from: the
/// release of the `easl` version in `Cargo.toml` and
/// `../editor/analysis/Cargo.toml` (checked by [`easl_dir`]).
const EASL_REPOSITORY: &str = "https://github.com/Ella-Hoeppner/easl.git";
const EASL_TAG: &str = "v0.2.0";

/// The easl_lsp repository, and the commit whose modules the editor's
/// analysis crate compiles in. They must build against the `easl` version
/// above.
const EASL_LSP_REPOSITORY: &str =
  "https://github.com/Ella-Hoeppner/easl_lsp.git";
const EASL_LSP_COMMIT: &str = "45ec1ed0e2733d40820527cef514dd1a09276827";

/// The variables naming local checkouts to build from instead.
const EASL_DIR_VARIABLE: &str = "EASL_DIR";
const EASL_LSP_DIR_VARIABLE: &str = "EASL_LSP_DIR";

/// Where pinned checkouts go, relative to the generator's manifest
/// directory.
const SOURCES_PATH: &str = "target/sources";

/// The file in a pinned checkout's `.git` directory recording the revision
/// it holds.
const REVISION_FILE: &str = "easl-site-revision";

/// The easl repository to build the web runtime from: `EASL_DIR`, or the
/// pinned release.
pub fn easl_dir(manifest_dir: &Path) -> PathBuf {
  if let Some(dir) = local(EASL_DIR_VARIABLE) {
    return dir;
  }
  check_crate_versions(manifest_dir);
  pinned(manifest_dir, "easl", EASL_REPOSITORY, EASL_TAG)
}

/// The easl_lsp repository whose modules the analysis crate compiles in:
/// `EASL_LSP_DIR`, or the pinned commit.
pub fn easl_lsp_dir(manifest_dir: &Path) -> PathBuf {
  local(EASL_LSP_DIR_VARIABLE).unwrap_or_else(|| {
    pinned(
      manifest_dir,
      "easl_lsp",
      EASL_LSP_REPOSITORY,
      EASL_LSP_COMMIT,
    )
  })
}

/// Whether this build uses local checkouts.
pub fn is_local() -> bool {
  local(EASL_DIR_VARIABLE).is_some() || local(EASL_LSP_DIR_VARIABLE).is_some()
}

fn local(variable: &str) -> Option<PathBuf> {
  let dir = PathBuf::from(std::env::var_os(variable)?);
  assert!(
    dir.is_dir(),
    "{variable} is {}, which isn't a directory",
    dir.display()
  );
  Some(dir)
}

/// Fails the build when a manifest's `easl` version isn't the release
/// [`EASL_TAG`] names, so the runtime and the compiler never come from two
/// different versions.
fn check_crate_versions(manifest_dir: &Path) {
  let wanted = EASL_TAG.trim_start_matches('v');
  for manifest in ["Cargo.toml", "../editor/analysis/Cargo.toml"] {
    let path = manifest_dir.join(manifest);
    let text = fs::read_to_string(&path)
      .unwrap_or_else(|e| panic!("couldn't read {}: {e}", path.display()));
    let line = format!("easl = \"{wanted}\"");
    assert!(
      text.lines().any(|l| l.trim() == line),
      "{} doesn't depend on easl {wanted}, the release the web runtime is \
       built from (sources.rs, EASL_TAG); keep them the same",
      path.display()
    );
  }
}

/// A checkout of `repository` at `revision` (a tag or a commit) in
/// `target/sources/<name>`, fetched when it's missing or holds another
/// revision.
fn pinned(
  manifest_dir: &Path,
  name: &str,
  repository: &str,
  revision: &str,
) -> PathBuf {
  let dir = manifest_dir.join(SOURCES_PATH).join(name);
  let record = dir.join(".git").join(REVISION_FILE);
  if fs::read_to_string(&record).is_ok_and(|held| held == revision) {
    return dir;
  }
  println!("fetching {name} {revision} from {repository}");
  if dir.exists() {
    fs::remove_dir_all(&dir).unwrap_or_else(|e| {
      panic!("couldn't remove the old {}: {e}", dir.display())
    });
  }
  fs::create_dir_all(&dir).unwrap();
  git(&dir, &["init", "--quiet"]);
  git(
    &dir,
    &["fetch", "--quiet", "--depth", "1", repository, revision],
  );
  git(&dir, &["checkout", "--quiet", "FETCH_HEAD"]);
  fs::write(&record, revision).unwrap();
  dir
}

fn git(dir: &Path, args: &[&str]) {
  let status = Command::new("git")
    .current_dir(dir)
    .args(args)
    .status()
    .unwrap_or_else(|e| panic!("couldn't run git: {e}"));
  assert!(
    status.success(),
    "git {} failed in {}",
    args.join(" "),
    dir.display()
  );
}
