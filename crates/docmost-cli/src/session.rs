//! Session helpers: JWT expiry inspection and the password store used for
//! silent re-login once the Docmost session token has expired.

use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use secrecy::{ExposeSecret, SecretString};

/// Seconds of clock skew tolerated before a token is treated as expired.
const EXPIRY_SKEW_SECONDS: u64 = 30;

/// Environment variable that swaps the OS keychain for a plaintext JSON file.
/// Only intended for automated tests; never document it as a user feature.
pub const PASSWORD_STORE_FILE_ENV: &str = "DOCMOST_PASSWORD_STORE_FILE";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u8;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(output)
}

/// Returns the `exp` claim of a JWT without validating its signature.
///
/// The value is only used to decide whether a request is worth attempting,
/// so trusting the unverified payload is safe: a forged `exp` merely
/// changes which request fails first.
pub fn jwt_expiry(token: &str) -> Option<u64> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64url_decode(payload)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let exp = value.get("exp")?;
    exp.as_u64()
        .or_else(|| exp.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
}

/// True only when the token carries an `exp` claim that has already passed
/// (allowing a small skew). Opaque tokens are never reported as expired.
pub fn is_expired(token: &str) -> bool {
    jwt_expiry(token).is_some_and(|exp| exp <= now() + EXPIRY_SKEW_SECONDS)
}

/// Where the login password is kept for silent re-login.
#[derive(Clone, Debug)]
pub enum PasswordStore {
    /// Platform credential store (macOS Keychain, Windows Credential
    /// Manager, or the Secret Service on other Unix systems).
    Keychain,
    /// Plaintext JSON file, selected through [`PASSWORD_STORE_FILE_ENV`].
    File(PathBuf),
}

impl PasswordStore {
    pub fn from_environment() -> Self {
        match std::env::var_os(PASSWORD_STORE_FILE_ENV) {
            Some(path) => PasswordStore::File(path.into()),
            None => PasswordStore::Keychain,
        }
    }

    fn service(api_url: &str) -> String {
        format!("docmost-cli:{api_url}")
    }

    pub fn get(&self, api_url: &str, username: &str) -> Result<Option<SecretString>, String> {
        match self {
            PasswordStore::Keychain => {
                let entry = keyring::Entry::new(&Self::service(api_url), username)
                    .map_err(keychain_error)?;
                match entry.get_password() {
                    Ok(password) => Ok(Some(SecretString::from(password))),
                    Err(keyring::Error::NoEntry) => Ok(None),
                    Err(error) => Err(keychain_error(error)),
                }
            }
            PasswordStore::File(path) => Ok(read_file_store(path)?
                .remove(&file_key(api_url, username))
                .map(SecretString::from)),
        }
    }

    pub fn set(
        &self,
        api_url: &str,
        username: &str,
        password: &SecretString,
    ) -> Result<(), String> {
        match self {
            PasswordStore::Keychain => keyring::Entry::new(&Self::service(api_url), username)
                .and_then(|entry| entry.set_password(password.expose_secret()))
                .map_err(keychain_error),
            PasswordStore::File(path) => {
                let mut entries = read_file_store(path)?;
                entries.insert(
                    file_key(api_url, username),
                    password.expose_secret().to_owned(),
                );
                write_file_store(path, &entries)
            }
        }
    }

    pub fn delete(&self, api_url: &str, username: &str) -> Result<(), String> {
        match self {
            PasswordStore::Keychain => {
                let entry = keyring::Entry::new(&Self::service(api_url), username)
                    .map_err(keychain_error)?;
                match entry.delete_credential() {
                    Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                    Err(error) => Err(keychain_error(error)),
                }
            }
            PasswordStore::File(path) => {
                let mut entries = read_file_store(path)?;
                entries.remove(&file_key(api_url, username));
                write_file_store(path, &entries)
            }
        }
    }
}

fn keychain_error(error: keyring::Error) -> String {
    format!("system keychain unavailable: {error}")
}

fn file_key(api_url: &str, username: &str) -> String {
    format!("{}\u{1f}{username}", PasswordStore::service(api_url))
}

fn read_file_store(path: &PathBuf) -> Result<BTreeMap<String, String>, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e.to_string()),
    }
}

fn write_file_store(path: &PathBuf, entries: &BTreeMap<String, String>) -> Result<(), String> {
    let data = serde_json::to_vec_pretty(entries).map_err(|e| e.to_string())?;
    fs::write(path, data).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_with_payload(payload: &str) -> String {
        let mut encoded = String::new();
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        for chunk in payload.as_bytes().chunks(3) {
            let mut buffer = [0u8; 3];
            buffer[..chunk.len()].copy_from_slice(chunk);
            let n = u32::from_be_bytes([0, buffer[0], buffer[1], buffer[2]]);
            let count = chunk.len() + 1;
            for i in 0..count {
                let shift = 18 - 6 * i;
                encoded.push(ALPHABET[((n >> shift) & 63) as usize] as char);
            }
        }
        format!("eyJhbGciOiJIUzI1NiJ9.{encoded}.signature")
    }

    #[test]
    fn reads_exp_claim_from_jwt() {
        assert_eq!(
            jwt_expiry(&token_with_payload(
                r#"{"token_type":"access","exp":1788462545}"#
            )),
            Some(1788462545)
        );
        assert_eq!(jwt_expiry(&token_with_payload(r#"{"exp":12.0}"#)), Some(12));
    }

    #[test]
    fn opaque_or_claimless_tokens_never_expire() {
        assert_eq!(jwt_expiry("opaque-token"), None);
        assert_eq!(jwt_expiry(&token_with_payload(r#"{"sub":"1"}"#)), None);
        assert_eq!(jwt_expiry("a.!!!.c"), None);
        assert!(!is_expired("opaque-token"));
    }

    #[test]
    fn expiry_respects_skew() {
        let past = token_with_payload(&format!(r#"{{"exp":{}}}"#, now() - 1));
        let soon = token_with_payload(&format!(r#"{{"exp":{}}}"#, now() + 5));
        let later = token_with_payload(&format!(r#"{{"exp":{}}}"#, now() + 3600));
        assert!(is_expired(&past));
        assert!(is_expired(&soon));
        assert!(!is_expired(&later));
    }

    /// Touches the real platform keychain; run explicitly with
    /// `cargo test -p docmost-cli -- --ignored keychain`.
    #[test]
    #[ignore]
    fn keychain_store_round_trips_passwords() {
        let store = PasswordStore::Keychain;
        let url = "https://keychain-test.invalid/api/v1";
        store.delete(url, "ada").unwrap();
        assert!(store.get(url, "ada").unwrap().is_none());
        store
            .set(url, "ada", &SecretString::from("s3cret"))
            .unwrap();
        assert_eq!(
            store
                .get(url, "ada")
                .unwrap()
                .map(|p| p.expose_secret().to_owned()),
            Some("s3cret".to_owned())
        );
        store.delete(url, "ada").unwrap();
        assert!(store.get(url, "ada").unwrap().is_none());
    }

    #[test]
    fn file_store_round_trips_passwords() {
        let directory = tempfile::tempdir().unwrap();
        let store = PasswordStore::File(directory.path().join("store.json"));
        assert!(store.get("https://a/api/v1", "ada").unwrap().is_none());
        store
            .set("https://a/api/v1", "ada", &SecretString::from("s3cret"))
            .unwrap();
        assert_eq!(
            store
                .get("https://a/api/v1", "ada")
                .unwrap()
                .map(|p| p.expose_secret().to_owned()),
            Some("s3cret".to_owned())
        );
        assert!(store.get("https://b/api/v1", "ada").unwrap().is_none());
        store.delete("https://a/api/v1", "ada").unwrap();
        assert!(store.get("https://a/api/v1", "ada").unwrap().is_none());
    }
}
