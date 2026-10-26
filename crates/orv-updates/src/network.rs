// SPDX-License-Identifier: GPL-3.0-or-later
use crate::protocol::{Manifest, REPOSITORY, Result, TARGET};
use reqwest::{blocking::Client, redirect::Policy};
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::Path,
    time::Duration,
};

fn allowed(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && matches!(
            url.host_str(),
            Some(
                "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
                    | "api.github.com"
            )
        )
}
fn client(version: &str, timeout: Duration) -> Result<Client> {
    Client::builder()
        .https_only(true)
        .no_proxy()
        .connect_timeout(Duration::from_secs(15))
        .timeout(timeout)
        .user_agent(format!("OpenRadView/{version}"))
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() >= 8 || !allowed(attempt.url()) {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| "Cannot initialize the update connection.")
}
fn status(response: &reqwest::blocking::Response) -> Result<()> {
    match response.status().as_u16() {
        200 => Ok(()),
        403 | 429 => Err("GitHub is limiting update requests. Try again later."),
        404 => Err("No public update package is available for this platform."),
        _ => Err("GitHub could not provide the update. Try again later."),
    }
}
pub(crate) fn manifest(version: &str) -> Result<Vec<u8>> {
    let response = client(version, Duration::from_secs(30))?
        .get(format!(
            "https://github.com/{REPOSITORY}/releases/latest/download/update-{TARGET}.json"
        ))
        .header("Accept", "application/json")
        .send()
        .map_err(|_| "Cannot reach GitHub for updates.")?;
    status(&response)?;
    if response.content_length().is_some_and(|n| n > 64 * 1024) {
        return Err("The update manifest is too large.");
    }
    let mut bytes = Vec::new();
    response
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read the update manifest.")?;
    if bytes.len() > 64 * 1024 {
        return Err("The update manifest is too large.");
    }
    Ok(bytes)
}
pub(crate) fn download(
    version: &str,
    manifest: &Manifest,
    path: &Path,
    mut progress: impl FnMut(u64),
) -> Result<()> {
    let mut response = client(version, Duration::from_secs(15 * 60))?
        .get(manifest.url())
        .header("Accept", "application/octet-stream")
        .send()
        .map_err(|_| "Cannot download the update from GitHub.")?;
    status(&response)?;
    if response
        .content_length()
        .is_some_and(|n| n != manifest.bytes)
    {
        return Err("The update download has an unexpected size.");
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "Cannot create the staged download.")?;
    transfer(&mut response, &mut output, manifest.bytes, &mut progress)?;
    output
        .sync_all()
        .map_err(|_| "Cannot finish saving the staged download.")?;
    manifest.verify_file(path)
}
pub(crate) fn transfer(
    input: &mut impl Read,
    output: &mut impl Write,
    size: u64,
    progress: &mut impl FnMut(u64),
) -> Result<()> {
    let mut total = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        let remaining = size - total;
        let cap = buffer.len().min(remaining.saturating_add(1) as usize);
        let n = input
            .read(&mut buffer[..cap])
            .map_err(|_| "The update download was interrupted.")?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > size {
            return Err("The update download exceeds its signed size.");
        }
        output
            .write_all(&buffer[..n])
            .map_err(|_| "Cannot write the staged download.")?;
        progress(total);
    }
    if total != size {
        return Err("The update download is incomplete.");
    }
    Ok(())
}
#[cfg(test)]
pub(crate) fn test_allowed(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| allowed(&u))
}
