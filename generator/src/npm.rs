//! The editor's JavaScript libraries (Monaco), which npm installs from
//! `package.json` and `package-lock.json` in the generator's directory into
//! its `node_modules/`, and the generator copies into the site.
//!
//! [`ensure_installed`] runs `npm ci` when `node_modules/` doesn't hold
//! exactly what the lockfile names, so a build never needs a manual install
//! step, and fails with the command to run when it can't.

use std::{fs, path::Path, process::Command};

use serde_json::Value;

/// The packages the lockfile names, as `(path in node_modules, version)`.
fn locked_packages(lockfile: &Value) -> Vec<(String, String)> {
  lockfile["packages"]
    .as_object()
    .into_iter()
    .flatten()
    .filter_map(|(path, entry)| {
      let path = path.strip_prefix("node_modules/")?;
      Some((path.to_string(), entry["version"].as_str()?.to_string()))
    })
    .collect()
}

/// The version of the package installed at `node_modules/<path>` in `dir`,
/// if one is.
pub fn installed_version(dir: &Path, path: &str) -> Option<String> {
  let manifest = dir.join("node_modules").join(path).join("package.json");
  let manifest: Value =
    serde_json::from_str(&fs::read_to_string(manifest).ok()?).ok()?;
  Some(manifest["version"].as_str()?.to_string())
}

fn read_json(path: &Path) -> Value {
  let text = fs::read_to_string(path)
    .unwrap_or_else(|e| panic!("couldn't read {}: {e}", path.display()));
  serde_json::from_str(&text)
    .unwrap_or_else(|e| panic!("{} isn't valid JSON: {e}", path.display()))
}

/// Whether `node_modules/` in `dir` lacks a package its `package-lock.json`
/// names, or has one at another version. Panics when `package.json` asks
/// for something the lockfile doesn't record (a dependency changed without
/// `npm install`).
fn needs_install(dir: &Path) -> bool {
  let package = read_json(&dir.join("package.json"));
  let lockfile = read_json(&dir.join("package-lock.json"));
  assert!(
    package["dependencies"] == lockfile["packages"][""]["dependencies"],
    "{}'s dependencies differ from package-lock.json's: run `npm install` in \
     {} to update the lockfile",
    dir.join("package.json").display(),
    dir.display(),
  );
  locked_packages(&lockfile)
    .into_iter()
    .any(|(path, version)| {
      installed_version(dir, &path).as_deref() != Some(version.as_str())
    })
}

/// Makes `node_modules/` in `dir` match its `package-lock.json`, running
/// `npm ci` when it doesn't (see [`needs_install`]). Panics when npm can't
/// run or fails.
pub fn ensure_installed(dir: &Path) {
  if !needs_install(dir) {
    return;
  }
  eprintln!("installing the editor's JavaScript libraries (npm ci)");
  let status = Command::new("npm")
    .args(["ci", "--no-audit", "--no-fund"])
    .current_dir(dir)
    .status()
    .unwrap_or_else(|e| {
      panic!(
        "couldn't run npm ({e}); install Node.js, then run `npm ci` in {}",
        dir.display()
      )
    });
  assert!(
    status.success(),
    "`npm ci` failed in {} (see its output above)",
    dir.display()
  );
}

#[cfg(test)]
mod tests {
  use std::{
    env, fs,
    path::{Path, PathBuf},
    process,
  };

  use super::needs_install;

  /// A fresh directory with a `package.json` asking for `wanted` and a
  /// lockfile recording `locked`, both versions of the package `lib`.
  fn project(name: &str, wanted: &str, locked: &str) -> PathBuf {
    let dir = env::temp_dir()
      .join(format!("easl-site-npm-test-{}-{name}", process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let dependencies = format!(r#"{{ "lib": "{wanted}" }}"#);
    fs::write(
      dir.join("package.json"),
      format!(r#"{{ "dependencies": {dependencies} }}"#),
    )
    .unwrap();
    fs::write(
      dir.join("package-lock.json"),
      format!(
        r#"{{ "packages": {{
          "": {{ "dependencies": {{ "lib": "{locked}" }} }},
          "node_modules/lib": {{ "version": "{locked}" }}
        }} }}"#
      ),
    )
    .unwrap();
    dir
  }

  fn install(dir: &Path, version: &str) {
    let package = dir.join("node_modules/lib");
    fs::create_dir_all(&package).unwrap();
    fs::write(
      package.join("package.json"),
      format!(r#"{{ "version": "{version}" }}"#),
    )
    .unwrap();
  }

  #[test]
  fn installs_what_is_missing_or_outdated() {
    let dir = project("current", "1.0.0", "1.0.0");
    assert!(needs_install(&dir), "nothing installed");
    install(&dir, "0.9.0");
    assert!(needs_install(&dir), "an older version installed");
    install(&dir, "1.0.0");
    assert!(!needs_install(&dir), "the locked version installed");
    fs::remove_dir_all(dir).unwrap();
  }

  #[test]
  #[should_panic(expected = "run `npm install`")]
  fn a_dependency_changed_without_npm_install_stops_the_build() {
    let dir = project("changed", "2.0.0", "1.0.0");
    install(&dir, "1.0.0");
    needs_install(&dir);
  }
}
