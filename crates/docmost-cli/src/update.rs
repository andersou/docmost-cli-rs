//! `update`: compares the running version with the latest GitHub
//! release, downloads the archive built for this platform, verifies it
//! against the release's `SHA256SUMS`, and swaps the current executable.

use std::{
    io::{Cursor, Read},
    path::PathBuf,
};

use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// GitHub repository that publishes the releases.
pub const OWNER: &str = "andersou";
pub const REPO: &str = "docmost-cli-rs";
/// Binary name inside the release archives.
pub const BIN_NAME: &str = "docmost-cli";
/// Environment variable that points at another releases API, for tests only.
pub const RELEASES_API_ENV: &str = "DOCMOST_CLI_RELEASES_API";
const DEFAULT_API: &str = "https://api.github.com";
/// Target triple this binary was built for, set by `build.rs`.
pub const TARGET: &str = env!("TARGET");
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("unable to reach the releases API: {0}")]
    Http(#[from] reqwest::Error),
    #[error("releases API answered {status}: {message}")]
    Api { status: u16, message: String },
    #[error("unable to parse the release: {0}")]
    Parse(String),
    #[error("release {tag} has no archive for {target}")]
    NoAsset { tag: String, target: String },
    #[error("release {tag} has no SHA256SUMS asset")]
    NoChecksums { tag: String },
    #[error("SHA256SUMS has no entry for {0}")]
    MissingChecksum(String),
    #[error("checksum mismatch for {name}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        name: String,
        expected: String,
        actual: String,
    },
    #[error("archive does not contain {0}")]
    MissingBinary(String),
    #[error("unable to read the archive: {0}")]
    Archive(String),
    #[error("unable to replace the executable: {0}")]
    Replace(#[from] std::io::Error),
}

#[derive(Clone, Debug, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn version(&self) -> Result<Version, UpdateError> {
        Version::parse(self.tag_name.trim_start_matches('v'))
            .map_err(|e| UpdateError::Parse(format!("tag {}: {e}", self.tag_name)))
    }
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|asset| asset.name == name)
    }
}

pub fn releases_api() -> String {
    std::env::var(RELEASES_API_ENV).unwrap_or_else(|_| DEFAULT_API.into())
}

/// Archive file name and extension produced by the CI packaging step.
pub fn archive_name(version: &Version, target: &str) -> (String, &'static str) {
    let ext = if target.contains("windows") {
        "zip"
    } else {
        "tar.gz"
    };
    (format!("{BIN_NAME}-{version}-{target}.{ext}"), ext)
}

pub fn binary_file_name(target: &str) -> String {
    if target.contains("windows") {
        format!("{BIN_NAME}.exe")
    } else {
        BIN_NAME.to_owned()
    }
}

pub fn http_client() -> Result<reqwest::Client, UpdateError> {
    Ok(reqwest::Client::builder()
        .user_agent(format!("{BIN_NAME}/{CURRENT_VERSION}"))
        .timeout(std::time::Duration::from_secs(300))
        .build()?)
}

pub async fn latest_release(http: &reqwest::Client) -> Result<Release, UpdateError> {
    let url = format!(
        "{}/repos/{OWNER}/{REPO}/releases/latest",
        releases_api().trim_end_matches('/')
    );
    let mut request = http
        .get(url)
        .header("accept", "application/vnd.github+json");
    // A token raises GitHub's unauthenticated rate limit; never required.
    if let Ok(token) = std::env::var("GITHUB_TOKEN")
        && !token.is_empty()
    {
        request = request.bearer_auth(token);
    }
    let response = request.send().await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        let message = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["message"].as_str().map(str::to_owned))
            .unwrap_or(text);
        return Err(UpdateError::Api {
            status: status.as_u16(),
            message,
        });
    }
    serde_json::from_str(&text).map_err(|e| UpdateError::Parse(e.to_string()))
}

pub async fn download(http: &reqwest::Client, url: &str) -> Result<Vec<u8>, UpdateError> {
    let response = http.get(url).send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(UpdateError::Api {
            status: status.as_u16(),
            message: format!("downloading {url}"),
        });
    }
    Ok(response.bytes().await?.to_vec())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Checks `bytes` against the `<hex>  <name>` line of a SHA256SUMS file.
pub fn verify_checksum(sums: &str, name: &str, bytes: &[u8]) -> Result<(), UpdateError> {
    let expected = sums
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let digest = parts.next()?;
            let file = parts.next()?.trim_start_matches('*');
            (file == name).then(|| digest.to_ascii_lowercase())
        })
        .next()
        .ok_or_else(|| UpdateError::MissingChecksum(name.to_owned()))?;
    let actual = hex(&Sha256::digest(bytes));
    if actual != expected {
        return Err(UpdateError::ChecksumMismatch {
            name: name.to_owned(),
            expected,
            actual,
        });
    }
    Ok(())
}

/// Pulls the binary out of a `.tar.gz` or `.zip` release archive.
pub fn extract_binary(archive: &[u8], ext: &str, file_name: &str) -> Result<Vec<u8>, UpdateError> {
    let matches = |path: &str| {
        path.trim_start_matches("./")
            .rsplit(['/', '\\'])
            .next()
            .is_some_and(|name| name == file_name)
    };
    if ext == "zip" {
        let mut zip = zip::ZipArchive::new(Cursor::new(archive))
            .map_err(|e| UpdateError::Archive(e.to_string()))?;
        for index in 0..zip.len() {
            let mut entry = zip
                .by_index(index)
                .map_err(|e| UpdateError::Archive(e.to_string()))?;
            if entry.is_file() && matches(entry.name()) {
                let mut bytes = Vec::new();
                entry
                    .read_to_end(&mut bytes)
                    .map_err(|e| UpdateError::Archive(e.to_string()))?;
                return Ok(bytes);
            }
        }
    } else {
        let decoder = flate2::read::GzDecoder::new(archive);
        let mut tar = tar::Archive::new(decoder);
        let entries = tar
            .entries()
            .map_err(|e| UpdateError::Archive(e.to_string()))?;
        for entry in entries {
            let mut entry = entry.map_err(|e| UpdateError::Archive(e.to_string()))?;
            let path = entry
                .path()
                .map_err(|e| UpdateError::Archive(e.to_string()))?
                .to_string_lossy()
                .into_owned();
            if entry.header().entry_type().is_file() && matches(&path) {
                let mut bytes = Vec::new();
                entry
                    .read_to_end(&mut bytes)
                    .map_err(|e| UpdateError::Archive(e.to_string()))?;
                return Ok(bytes);
            }
        }
    }
    Err(UpdateError::MissingBinary(file_name.to_owned()))
}

/// Writes the new binary next to the running one and swaps them in place.
pub fn install(bytes: &[u8]) -> Result<PathBuf, UpdateError> {
    let current = std::env::current_exe()?;
    let staged = current.with_extension(format!("{}.new", std::process::id()));
    std::fs::write(&staged, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }
    let replaced = self_replace::self_replace(&staged);
    let _ = std::fs::remove_file(&staged);
    replaced?;
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_archives_like_the_release_workflow() {
        let version = Version::parse("1.2.3").unwrap();
        assert_eq!(
            archive_name(&version, "x86_64-unknown-linux-gnu"),
            (
                "docmost-cli-1.2.3-x86_64-unknown-linux-gnu.tar.gz".into(),
                "tar.gz"
            )
        );
        assert_eq!(
            archive_name(&version, "x86_64-pc-windows-msvc"),
            ("docmost-cli-1.2.3-x86_64-pc-windows-msvc.zip".into(), "zip")
        );
        assert_eq!(
            binary_file_name("x86_64-pc-windows-msvc"),
            "docmost-cli.exe"
        );
        assert_eq!(binary_file_name("aarch64-apple-darwin"), "docmost-cli");
    }

    #[test]
    fn verifies_sha256sums_entries() {
        let digest = hex(&Sha256::digest(b"payload"));
        let sums = format!("{digest}  a.tar.gz\n0000  b.zip\n");
        verify_checksum(&sums, "a.tar.gz", b"payload").unwrap();
        assert!(matches!(
            verify_checksum(&sums, "a.tar.gz", b"tampered"),
            Err(UpdateError::ChecksumMismatch { .. })
        ));
        assert!(matches!(
            verify_checksum(&sums, "c.zip", b"payload"),
            Err(UpdateError::MissingChecksum(_))
        ));
    }

    #[test]
    fn extracts_the_binary_from_both_archive_kinds() {
        let mut tar_bytes = Vec::new();
        {
            let encoder =
                flate2::write::GzEncoder::new(&mut tar_bytes, flate2::Compression::fast());
            let mut builder = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_size(5);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, "docmost-cli", &b"hello"[..])
                .unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        assert_eq!(
            extract_binary(&tar_bytes, "tar.gz", "docmost-cli").unwrap(),
            b"hello"
        );
        assert!(matches!(
            extract_binary(&tar_bytes, "tar.gz", "other"),
            Err(UpdateError::MissingBinary(_))
        ));

        let mut zip_bytes = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut zip_bytes));
            writer
                .start_file("docmost-cli.exe", zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut writer, b"world").unwrap();
            writer.finish().unwrap();
        }
        assert_eq!(
            extract_binary(&zip_bytes, "zip", "docmost-cli.exe").unwrap(),
            b"world"
        );
    }

    #[test]
    fn parses_release_versions() {
        let release = Release {
            tag_name: "v1.4.0".into(),
            html_url: String::new(),
            assets: Vec::new(),
        };
        assert_eq!(release.version().unwrap(), Version::new(1, 4, 0));
    }
}
