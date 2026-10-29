use crate::paths::{remove_if_exists, Paths};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;

/// The shared secret clients must present in `Hello`: 64 hex characters.
pub struct Token(String);

impl Token {
    /// Constant-time comparison; only the length can leak.
    pub fn verify(&self, candidate: &str) -> bool {
        let expected = self.0.as_bytes();
        let candidate = candidate.as_bytes();
        expected.len() == candidate.len()
            && expected
                .iter()
                .zip(candidate)
                .fold(0, |difference, (a, b)| difference | (a ^ b))
                == 0
    }
}

/// Write a fresh random token to `daemon.token` (0600). Call only while holding the
/// instance lock.
pub fn create_token(paths: &Paths) -> io::Result<Token> {
    let mut bytes = [0_u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();

    // Replace the file instead of truncating it so a stale file's mode cannot apply.
    remove_if_exists(&paths.token())?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(paths.token())?
        .write_all(token.as_bytes())?;
    Ok(Token(token))
}

#[cfg(test)]
mod tests {
    use super::create_token;
    use crate::paths::Paths;
    use crate::test_support::TempDir;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn token_file_is_0600_with_64_hex_characters() {
        let temp = TempDir::new("auth-file");
        let paths = Paths::at(temp.path()).unwrap();

        create_token(&paths).unwrap();

        let mode = std::fs::metadata(paths.token())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let text = std::fs::read_to_string(paths.token()).unwrap();
        assert_eq!(text.len(), 64);
        assert!(text.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn two_tokens_differ_and_the_file_holds_the_latest() {
        let temp = TempDir::new("auth-differ");
        let paths = Paths::at(temp.path()).unwrap();

        let first = create_token(&paths).unwrap();
        let first_text = std::fs::read_to_string(paths.token()).unwrap();
        let second = create_token(&paths).unwrap();
        let second_text = std::fs::read_to_string(paths.token()).unwrap();

        assert_ne!(first_text, second_text);
        assert!(first.verify(&first_text));
        assert!(second.verify(&second_text));
        assert!(!second.verify(&first_text));
    }

    #[test]
    fn verify_rejects_wrong_and_different_length_values() {
        let temp = TempDir::new("auth-verify");
        let paths = Paths::at(temp.path()).unwrap();
        let token = create_token(&paths).unwrap();
        let text = std::fs::read_to_string(paths.token()).unwrap();

        assert!(token.verify(&text));
        assert!(!token.verify(&"0".repeat(64)));
        assert!(!token.verify(&text[..63]));
        assert!(!token.verify(&format!("{text}0")));
        assert!(!token.verify(""));
    }
}
