// SPDX-License-Identifier: GPL-3.0-or-later
use base64::{Engine, prelude::BASE64_STANDARD};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};

pub(crate) type Result<T> = std::result::Result<T, &'static str>;
pub(crate) const TARGET: &str = env!("ORV_UPDATE_TARGET");
pub(crate) const REPOSITORY: &str = "gyorgysh/openradview";
pub(crate) const MAX_ARCHIVE: u64 = 512 * 1024 * 1024;
pub(crate) const MAX_EXPANDED: u64 = 2 * 1024 * 1024 * 1024;
pub(crate) const MAX_FILES: usize = 8192;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    manifest: String,
    signature: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub schema_version: u32,
    pub repository: String,
    pub version: String,
    pub target: String,
    pub archive: String,
    pub bundle: String,
    pub bytes: u64,
    pub sha256: String,
}

pub(crate) fn supported_target(target: &str) -> bool {
    matches!(
        target,
        "aarch64-apple-darwin"
            | "x86_64-apple-darwin"
            | "x86_64-pc-windows-gnu"
            | "x86_64-pc-windows-msvc"
            | "aarch64-pc-windows-msvc"
            | "x86_64-unknown-linux-gnu"
            | "aarch64-unknown-linux-gnu"
    )
}
pub(crate) fn stable_version(value: &str) -> Result<semver::Version> {
    if value.len() > 40
        || value.split('.').count() != 3
        || !value.bytes().all(|b| b.is_ascii_digit() || b == b'.')
    {
        return Err("The update version is invalid.");
    }
    semver::Version::parse(value).map_err(|_| "The update version is invalid.")
}
pub(crate) fn names(version: &str, target: &str) -> (String, String) {
    if target.ends_with("apple-darwin") {
        let arch = if target.starts_with("aarch64") {
            "arm64"
        } else {
            "x86_64"
        };
        (
            format!("OpenRadView-{version}-macos-{arch}-standard.zip"),
            "OpenRadView.app".into(),
        )
    } else {
        let stem = format!("OpenRadView-{version}-{target}");
        (
            format!(
                "{stem}{}",
                if target.contains("windows") {
                    ".zip"
                } else {
                    ".tar.gz"
                }
            ),
            stem,
        )
    }
}
pub(crate) fn public_key() -> Result<VerifyingKey> {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../../../packaging/updater.json"))
            .map_err(|_| "Invalid updater configuration.")?;
    if config["repository"] != REPOSITORY {
        return Err("Invalid updater repository.");
    }
    let bytes = BASE64_STANDARD
        .decode(
            config["public_ed25519_key"]
                .as_str()
                .ok_or("Missing update public key.")?,
        )
        .map_err(|_| "Invalid update public key.")?;
    VerifyingKey::from_bytes(&bytes.try_into().map_err(|_| "Invalid update public key.")?)
        .map_err(|_| "Invalid update public key.")
}
pub(crate) fn verify_envelope(bytes: &[u8], key: &VerifyingKey, target: &str) -> Result<Manifest> {
    if bytes.len() > 64 * 1024 {
        return Err("The update manifest is too large.");
    }
    let envelope: Envelope =
        serde_json::from_slice(bytes).map_err(|_| "The update manifest is invalid.")?;
    let raw = BASE64_STANDARD
        .decode(envelope.manifest)
        .map_err(|_| "The signed manifest is invalid.")?;
    let signature = BASE64_STANDARD
        .decode(envelope.signature)
        .map_err(|_| "The update signature is invalid.")?;
    let signature =
        Signature::from_slice(&signature).map_err(|_| "The update signature is invalid.")?;
    key.verify_strict(&raw, &signature)
        .map_err(|_| "The update signature could not be verified.")?;
    let manifest: Manifest =
        serde_json::from_slice(&raw).map_err(|_| "The signed manifest is invalid.")?;
    stable_version(&manifest.version)?;
    let expected = names(&manifest.version, target);
    if manifest.schema_version != 1
        || manifest.repository != REPOSITORY
        || manifest.target != target
        || !supported_target(target)
        || manifest.archive != expected.0
        || manifest.bundle != expected.1
        || manifest.bytes == 0
        || manifest.bytes > MAX_ARCHIVE
        || manifest.sha256.len() != 64
        || !manifest
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("The signed update does not match this application and platform.");
    }
    Ok(manifest)
}
impl Manifest {
    pub fn url(&self) -> String {
        format!(
            "https://github.com/{REPOSITORY}/releases/download/v{}/{}",
            self.version, self.archive
        )
    }
    pub fn verify_file(&self, path: &Path) -> Result<()> {
        let mut file = File::open(path).map_err(|_| "Cannot read the staged download.")?;
        if file
            .metadata()
            .map_err(|_| "Cannot inspect the staged download.")?
            .len()
            != self.bytes
        {
            return Err("The update download has an unexpected size.");
        }
        let mut hash = Sha256::new();
        let mut buf = [0; 64 * 1024];
        loop {
            let n = file
                .read(&mut buf)
                .map_err(|_| "Cannot verify the staged download.")?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
        }
        if hash
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
            != self.sha256
        {
            return Err("The update checksum could not be verified.");
        }
        Ok(())
    }
}
