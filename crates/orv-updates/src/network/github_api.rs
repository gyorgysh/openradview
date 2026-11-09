// SPDX-License-Identifier: GPL-3.0-or-later
//! Public GitHub asset transport when the web download route returns a server error.
use super::{Client, Manifest, REPOSITORY, Result, TARGET, status};
use crate::protocol::stable_version;
use reqwest::blocking::Response;
use serde::Deserialize;
use std::{io::Read, time::Duration};

const METADATA_LIMIT: u64 = 512 * 1024;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    id: u64,
    name: String,
}

fn asset_url(bytes: &[u8], name: &str, version: Option<&str>) -> Result<String> {
    if bytes.len() as u64 > METADATA_LIMIT {
        return Err("GitHub release metadata is too large.");
    }
    let release: Release =
        serde_json::from_slice(bytes).map_err(|_| "GitHub returned invalid release metadata.")?;
    let tag_version = release
        .tag_name
        .strip_prefix('v')
        .ok_or("GitHub returned an unexpected release version.")?;
    stable_version(tag_version)?;
    if version.is_some_and(|version| version != tag_version) || release.assets.len() > 128 {
        return Err("GitHub returned unexpected release metadata.");
    }
    let mut matches = release.assets.iter().filter(|asset| asset.name == name);
    let asset = matches
        .next()
        .filter(|asset| asset.id != 0)
        .ok_or("No public update package is available for this platform.")?;
    if matches.next().is_some() {
        return Err("GitHub returned ambiguous update assets.");
    }
    // Do not follow URLs supplied by release metadata. The signed envelope and
    // archive hash remain authoritative; this selects only a public transport.
    Ok(format!(
        "https://api.github.com/repos/{REPOSITORY}/releases/assets/{}",
        asset.id
    ))
}

fn fetch(client: &Client, release: &str, name: &str, version: Option<&str>) -> Result<Response> {
    let response = client
        .get(format!(
            "https://api.github.com/repos/{REPOSITORY}/releases/{release}"
        ))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2026-03-10")
        .timeout(Duration::from_secs(30))
        .send()
        .map_err(|_| "Cannot reach GitHub for updates.")?;
    status(&response)?;
    if response
        .content_length()
        .is_some_and(|n| n > METADATA_LIMIT)
    {
        return Err("GitHub release metadata is too large.");
    }
    let mut bytes = Vec::new();
    response
        .take(METADATA_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read GitHub release metadata.")?;
    let url = asset_url(&bytes, name, version)?;
    client
        .get(url)
        .header("Accept", "application/octet-stream")
        .header("X-GitHub-Api-Version", "2026-03-10")
        .send()
        .map_err(|_| "Cannot download the update from GitHub.")
}

pub(super) fn manifest(client: &Client) -> Result<Response> {
    fetch(client, "latest", &format!("update-{TARGET}.json"), None)
}

pub(super) fn archive(client: &Client, manifest: &Manifest) -> Result<Response> {
    stable_version(&manifest.version)?;
    fetch(
        client,
        &format!("tags/v{}", manifest.version),
        &manifest.archive,
        Some(&manifest.version),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn asset_lookup_uses_only_the_fixed_repository_and_numeric_id() {
        let bytes = serde_json::to_vec(&json!({
            "tag_name":"v1.3.0", "assets":[{"id":42,"name":"update.json",
            "url":"https://untrusted.invalid/other", "browser_download_url":"file:///other"}]
        }))
        .unwrap();
        assert_eq!(
            asset_url(&bytes, "update.json", Some("1.3.0")).unwrap(),
            format!("https://api.github.com/repos/{REPOSITORY}/releases/assets/42")
        );
    }

    #[test]
    fn asset_lookup_rejects_wrong_or_noncanonical_release_tags() {
        for tag in ["v1.2.0", "v1.3.0-rc.1", "v1.3.0/other", "1.3.0"] {
            let bytes = serde_json::to_vec(&json!({
                "tag_name":tag,"assets":[{"id":42,"name":"update.json"}]
            }))
            .unwrap();
            assert!(asset_url(&bytes, "update.json", Some("1.3.0")).is_err());
        }
    }

    #[test]
    fn asset_lookup_rejects_ambiguous_missing_and_zero_id_assets() {
        for assets in [
            json!([]),
            json!([{"id":0,"name":"update.json"}]),
            json!([{"id":1,"name":"other"}]),
            json!([{"id":1,"name":"update.json"},{"id":2,"name":"update.json"}]),
        ] {
            let bytes = serde_json::to_vec(&json!({"tag_name":"v1.3.0","assets":assets})).unwrap();
            assert!(asset_url(&bytes, "update.json", None).is_err());
        }
    }

    #[test]
    fn asset_lookup_bounds_metadata_and_asset_count() {
        assert!(
            asset_url(
                &vec![b' '; METADATA_LIMIT as usize + 1],
                "update.json",
                None
            )
            .is_err()
        );
        let assets = (1..=129)
            .map(|id| json!({"id":id,"name":"other"}))
            .collect::<Vec<_>>();
        let bytes = serde_json::to_vec(&json!({"tag_name":"v1.3.0","assets":assets})).unwrap();
        assert!(asset_url(&bytes, "update.json", None).is_err());
    }
}
