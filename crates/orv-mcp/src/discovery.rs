// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-user rendezvous metadata only. A live lock excludes crashed/stale records.
//! Discovery is not authentication; desktop approval still creates every grant.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const LIMIT: u64 = 4096;
const APP: &str = "OpenRadView";
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Instance {
    pub schema_version: u32,
    pub app: String,
    pub instance_id: String,
    pub pid: u32,
    pub version: String,
    pub port: u16,
}
impl Instance {
    pub(crate) fn new(port: u16) -> Result<Self, &'static str> {
        let mut random = [0; 32];
        getrandom::fill(&mut random).map_err(|_| "Could not identify this app instance")?;
        Ok(Self {
            schema_version: 1,
            app: APP.into(),
            instance_id: random.iter().map(|b| format!("{b:02x}")).collect(),
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").into(),
            port,
        })
    }
    fn valid(&self) -> bool {
        self.schema_version == 1
            && self.app == APP
            && orv_core::scene::is_sha256(&self.instance_id)
            && self.pid != 0
            && self.port != 0
            && self.version.len() <= 32
            && self.version.split('.').count() == 3
            && self
                .version
                .split('.')
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    }
    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}/request", self.port)
    }
}
#[derive(Clone)]
pub(crate) struct Registry {
    folder: PathBuf,
}
impl Registry {
    #[cfg(test)]
    pub(crate) fn at(folder: PathBuf) -> Self {
        Self { folder }
    }
    pub(crate) fn local() -> Result<Self, &'static str> {
        let base = if cfg!(target_os = "windows") {
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        } else if cfg!(target_os = "macos") {
            std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Library/Caches"))
        } else {
            std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".cache")))
        }
        .filter(|p| p.is_absolute())
        .ok_or("Local app discovery folder unavailable")?;
        Ok(Self {
            folder: base.join(APP).join("mcp-instances"),
        })
    }
    fn check(&self, create: bool) -> Result<(), &'static str> {
        if create {
            fs::create_dir_all(self.folder.parent().ok_or("Invalid discovery folder")?)
                .map_err(|_| "Cannot create app discovery folder")?;
            let builder = fs::DirBuilder::new();
            #[cfg(unix)]
            let builder = {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = builder;
                builder.mode(0o700);
                builder
            };
            if let Err(e) = builder.create(&self.folder)
                && e.kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err("Cannot create app discovery folder");
            }
        }
        let meta = fs::symlink_metadata(&self.folder)
            .map_err(|_| "Local app discovery folder unavailable")?;
        if !meta.is_dir() || meta.is_symlink() {
            return Err("Unsafe app discovery folder");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let home = std::env::var_os("HOME")
                .and_then(|p| fs::metadata(p).ok())
                .ok_or("Local user unavailable")?;
            if meta.mode() & 0o077 != 0 || meta.uid() != home.uid() {
                return Err("App discovery folder must be private to this user");
            }
        }
        Ok(())
    }
    pub(crate) fn publish(&self, instance: Instance) -> Result<Lease, &'static str> {
        self.check(true)?;
        let lock_path = self.folder.join(format!("{}.lock", instance.instance_id));
        let path = self.folder.join(format!("{}.json", instance.instance_id));
        let lock = private_create(&lock_path)?;
        if lock.try_lock().is_err() {
            let _ = fs::remove_file(&lock_path);
            return Err("Cannot lock app discovery record");
        }
        let bytes =
            serde_json::to_vec(&instance).map_err(|_| "Cannot encode app discovery record")?;
        let mut lease = Lease {
            lock: Some(lock),
            lock_path,
            path,
            bytes,
            published: false,
        };
        let mut file = private_create(&lease.path)?;
        lease.published = true;
        file.write_all(&lease.bytes)
            .map_err(|_| "Cannot write app discovery record")?;
        Ok(lease)
    }
    pub(crate) fn list(&self) -> Result<Vec<Instance>, &'static str> {
        if !self
            .folder
            .try_exists()
            .map_err(|_| "App discovery unavailable")?
        {
            return Ok(Vec::new());
        }
        self.check(false)?;
        let mut found = Vec::new();
        let entries = fs::read_dir(&self.folder).map_err(|_| "App discovery unavailable")?;
        for (n, entry) in entries.enumerate() {
            if n >= 512 {
                return Err(
                    "Too many discovery records; close apps and clear stale local discovery files",
                );
            }
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !orv_core::scene::is_sha256(stem) {
                continue;
            }
            let lock_path = path.with_extension("lock");
            let Ok(lock) = private_read(&lock_path) else {
                continue;
            };
            if !matches!(lock.try_lock(), Err(TryLockError::WouldBlock)) {
                continue;
            }
            let Ok(file) = private_read(&path) else {
                continue;
            };
            let mut bytes = Vec::new();
            if file.take(LIMIT + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > LIMIT {
                continue;
            }
            let Ok(instance) = serde_json::from_slice::<Instance>(&bytes) else {
                continue;
            };
            if instance.valid() && instance.instance_id == stem {
                found.push(instance);
                if found.len() > 32 {
                    return Err("Too many running app instances");
                }
            }
        }
        found.sort_by(|a, b| a.instance_id.cmp(&b.instance_id));
        Ok(found)
    }
}
fn private_create(path: &Path) -> Result<File, &'static str> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|_| "Cannot create private discovery record")
}
fn private_read(path: &Path) -> Result<File, &'static str> {
    let before = fs::symlink_metadata(path).map_err(|_| "Discovery record unavailable")?;
    if !before.is_file() || before.is_symlink() || before.len() > LIMIT {
        return Err("Invalid discovery record");
    }
    // Write access is necessary for an exclusive lock on Windows. No content is written.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|_| "Discovery record unavailable")?;
    let after = file
        .metadata()
        .map_err(|_| "Discovery record unavailable")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let parent = fs::symlink_metadata(path.parent().ok_or("Invalid discovery record")?)
            .map_err(|_| "Discovery folder unavailable")?;
        if before.dev() != after.dev()
            || before.ino() != after.ino()
            || after.uid() != parent.uid()
            || after.mode() & 0o077 != 0
            || after.nlink() != 1
        {
            return Err("Unsafe discovery record");
        }
    }
    if !after.is_file() || after.len() > LIMIT {
        return Err("Invalid discovery record");
    }
    Ok(file)
}
pub(crate) struct Lease {
    lock: Option<File>,
    lock_path: PathBuf,
    path: PathBuf,
    bytes: Vec<u8>,
    published: bool,
}
impl Drop for Lease {
    fn drop(&mut self) {
        // Never remove an altered/replaced record. A crash leaves only nonsecret
        // metadata; without the live lock it cannot be selected by discovery.
        let bytes = private_read(&self.path).ok().and_then(|f| {
            let mut b = Vec::new();
            f.take(LIMIT + 1).read_to_end(&mut b).ok()?;
            Some(b)
        });
        if self.published && bytes.as_deref() == Some(&self.bytes) {
            let _ = fs::remove_file(&self.path);
        }
        self.lock.take();
        let _ = fs::remove_file(&self.lock_path);
    }
}
pub fn instances() -> Result<Vec<Instance>, &'static str> {
    Registry::local()?.list()
}
#[cfg(test)]
mod tests;
