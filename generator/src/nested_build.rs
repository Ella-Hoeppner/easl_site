//! Running other builds (the web runtime, the playground's analysis crate)
//! from inside the generator's own `cargo run`.

use std::{ffi::OsStr, process::Command};

/// A command that runs without this process's cargo environment: `cargo
/// run` sets variables describing the generator's own build, which would
/// leak into a nested build's build scripts.
pub fn command(program: impl AsRef<OsStr>) -> Command {
  let mut command = Command::new(program);
  for (name, _) in std::env::vars_os() {
    let name = name.to_string_lossy();
    if (name.starts_with("CARGO") && name != "CARGO_HOME") || name == "TARGET" {
      command.env_remove(name.as_ref());
    }
  }
  command
}
