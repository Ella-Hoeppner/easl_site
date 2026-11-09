//! Secret tokens (session cookies, email links) and their stored hashes.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// A fresh secret: 32 random bytes, base64url-encoded without padding.
pub fn new_token() -> String {
  let mut bytes = [0u8; 32];
  rand::rng().fill_bytes(&mut bytes);
  URL_SAFE_NO_PAD.encode(bytes)
}

/// The hex SHA-256 of a token, which is all the database stores. Tokens are
/// looked up by this hash, so a stolen database can't be replayed and no
/// secret is ever compared byte by byte.
pub fn hash_token(token: &str) -> String {
  let digest = Sha256::digest(token.as_bytes());
  let mut hex = String::with_capacity(64);
  for byte in digest {
    hex.push_str(&format!("{byte:02x}"));
  }
  hex
}

/// Whether a string has the shape of a token from [`new_token`]: cheap
/// rejection of garbage before any database work.
pub fn is_token_shaped(token: &str) -> bool {
  token.len() == 43
    && token
      .bytes()
      .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn tokens_are_distinct_and_shaped() {
    let a = new_token();
    let b = new_token();
    assert_ne!(a, b);
    assert!(is_token_shaped(&a));
    assert_eq!(hash_token(&a).len(), 64);
    assert!(!is_token_shaped("short"));
  }
}
