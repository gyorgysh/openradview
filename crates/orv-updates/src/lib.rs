// SPDX-License-Identifier: GPL-3.0-or-later
//! Optional, anonymous GitHub update traffic only. No study, catalog or UI dependencies.
mod archive;
mod install;
mod lifecycle;
mod network;
mod preferences;
mod protocol;
#[cfg(feature = "release-validation")]
pub mod release_validation;
#[cfg(test)]
mod tests;
use preferences::{Cleanup, Preferences};
use protocol::{Manifest, Result, TARGET};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Unavailable,
    Idle,
    Checking,
    Current,
    Available,
    Downloading,
    Verifying,
    Installing,
    ReadyToRestart,
    Failed,
}
impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "Updates are unavailable in this build.",
            Self::Idle => "Ready to check for updates.",
            Self::Checking => "Checking GitHub for updates…",
            Self::Current => "No newer update was found for this platform.",
            Self::Available => "An update is available.",
            Self::Downloading => "Downloading the update you chose…",
            Self::Verifying => "Verifying the update you chose…",
            Self::Installing => "Installing the update you chose…",
            Self::ReadyToRestart => "The update is installed. Restart when you are ready.",
            Self::Failed => "The update could not be completed.",
        }
    }
    pub fn busy(self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading | Self::Verifying | Self::Installing
        )
    }
}
#[derive(Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub bytes: u64,
}
struct State {
    status: Status,
    detail: &'static str,
    manifest: Option<Manifest>,
    downloaded: u64,
    installable: bool,
    package_managed: bool,
    prefs: Preferences,
}
enum Task {
    Check,
    Install,
    Wake,
    Stop,
}
pub struct Updater {
    state: Arc<Mutex<State>>,
    tasks: Option<mpsc::SyncSender<Task>>,
    prefs_path: Option<PathBuf>,
    executable: Option<PathBuf>,
    restart: bool,
    lifecycle: Arc<lifecycle::Lifecycle>,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
fn lock(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn change(state: &Mutex<State>, notify: &dyn Fn(), status: Status, detail: &'static str) {
    {
        let mut state = lock(state);
        state.status = status;
        state.detail = detail;
    }
    notify();
}
impl Updater {
    pub fn new(notify: Arc<dyn Fn() + Send + Sync>, version: &str) -> Self {
        let lifecycle = Arc::new(lifecycle::Lifecycle::default());
        let prefs_path = preferences::location();
        let prefs = prefs_path
            .as_deref()
            .map(preferences::load)
            .unwrap_or_default();
        let executable = std::env::current_exe()
            .ok()
            .and_then(|p| p.canonicalize().ok());
        let package_managed = executable.as_deref().is_some_and(package_install);
        let available = protocol::supported_target(TARGET)
            && prefs_path.is_some()
            && executable.is_some()
            && protocol::public_key().is_ok();
        let state = Arc::new(Mutex::new(State {
            status: if available {
                Status::Idle
            } else {
                Status::Unavailable
            },
            detail: "",
            manifest: None,
            downloaded: 0,
            installable: false,
            package_managed,
            prefs,
        }));
        let tasks = if available {
            let (send, receive) = mpsc::sync_channel(1);
            let shared = state.clone();
            let version = version.to_owned();
            let path = prefs_path.clone();
            let exe = executable.clone();
            let lifecycle = lifecycle.clone();
            let spawned = std::thread::Builder::new()
                .name("orv-updates".into())
                .spawn(move || {
                    if let (Some(path), Some(exe)) = (path, exe) {
                        worker(
                            &shared, &*notify, &path, &exe, &version, receive, &lifecycle,
                        );
                    }
                });
            if spawned.is_ok() {
                Some(send)
            } else {
                lock(&state).status = Status::Unavailable;
                None
            }
        } else {
            None
        };
        Self {
            state,
            tasks,
            prefs_path,
            executable,
            restart: false,
            lifecycle,
        }
    }
    pub fn available(&self) -> bool {
        self.tasks.is_some()
    }
    pub fn preference_chosen(&self) -> bool {
        lock(&self.state).prefs.chosen
    }
    pub fn enabled(&self) -> bool {
        let state = lock(&self.state);
        !state.package_managed && state.prefs.enabled
    }
    pub fn package_managed(&self) -> bool {
        lock(&self.state).package_managed
    }
    pub fn set_enabled(&mut self, enabled: bool) -> Result<()> {
        if self.package_managed() {
            return Err(
                "Automatic GitHub checks are off for this package-managed installation. You can check manually.",
            );
        }
        let path = self
            .prefs_path
            .as_deref()
            .ok_or("Cannot locate update preferences.")?;
        let mut state = lock(&self.state);
        let mut prefs = state.prefs.clone();
        prefs.chosen = true;
        prefs.enabled = enabled;
        if preferences::save(path, &prefs).is_err() {
            state.prefs.enabled = false;
            state.prefs.chosen = false;
            state.status = Status::Failed;
            state.detail = "Cannot save update preferences. Automatic checks are off for this run.";
            return Err(state.detail);
        }
        state.prefs = prefs;
        drop(state);
        if let Some(sender) = &self.tasks {
            let _ = sender.try_send(Task::Wake);
        }
        Ok(())
    }
    fn send(&self, task: Task) -> Result<()> {
        let state = lock(&self.state);
        if state.status.busy() {
            return Err("An update task is already running.");
        }
        if state.status == Status::ReadyToRestart {
            return Err("Restart the app before checking for another update.");
        }
        drop(state);
        self.tasks
            .as_ref()
            .ok_or("Updater unavailable.")?
            .try_send(task)
            .map_err(|_| "An update task is already queued.")
    }
    pub fn check(&mut self) -> Result<()> {
        self.send(Task::Check)
    }
    pub fn install(&mut self) -> Result<()> {
        if lock(&self.state).status != Status::Available {
            return Err("Check for an available update first.");
        }
        if !self.can_install() {
            return Err(self.detail());
        }
        self.send(Task::Install)
    }
    pub fn status(&self) -> Status {
        lock(&self.state).status
    }
    pub fn detail(&self) -> &'static str {
        lock(&self.state).detail
    }
    pub fn update_info(&self) -> Option<UpdateInfo> {
        lock(&self.state).manifest.as_ref().map(|m| UpdateInfo {
            version: m.version.clone(),
            bytes: m.bytes,
        })
    }
    pub fn can_install(&self) -> bool {
        lock(&self.state).installable
    }
    pub fn downloaded_bytes(&self) -> u64 {
        lock(&self.state).downloaded
    }
    /// False only during the short rename/rollback section. Retrying after
    /// that section closes normally; a quit that wins first cancels the swap.
    pub fn request_close(&self) -> bool {
        self.lifecycle.request_close()
    }
    pub fn request_restart(&mut self) {
        if self.status() == Status::ReadyToRestart {
            self.restart = true;
        }
    }
    /// Only the explicit Restart action reaches this path. The host first
    /// cancels work and revokes agent access, after saving/discarding labels.
    pub fn finish_relaunch(&mut self, before: impl FnOnce()) {
        if !std::mem::take(&mut self.restart) {
            return;
        }
        before();
        let result = self
            .executable
            .as_deref()
            .ok_or("Cannot locate the updated app.")
            .and_then(install::relaunch);
        if let Err(message) = result {
            let mut state = lock(&self.state);
            state.status = Status::Failed;
            state.detail = message;
        } else {
            std::process::exit(0);
        }
    }
}
impl Drop for Updater {
    fn drop(&mut self) {
        if let Some(sender) = &self.tasks {
            let _ = sender.try_send(Task::Stop);
        }
    }
}

fn check(version: &str) -> Result<Option<Manifest>> {
    let bytes = network::manifest(version)?;
    let manifest = protocol::verify_envelope(&bytes, &protocol::public_key()?, TARGET)?;
    if protocol::stable_version(&manifest.version)? > protocol::stable_version(version)? {
        Ok(Some(manifest))
    } else {
        Ok(None)
    }
}
fn perform_install(
    state: &Mutex<State>,
    notify: &dyn Fn(),
    exe: &std::path::Path,
    version: &str,
    manifest: &Manifest,
    lifecycle: &lifecycle::Lifecycle,
) -> Result<PathBuf> {
    let current = install::root(exe)?;
    install::current_trust(&current, Some(version), || lifecycle.closing())?;
    let parent = current
        .parent()
        .ok_or("Cannot locate the app's parent folder.")?;
    let staging = tempfile::Builder::new()
        .prefix(".openradview-update-stage-")
        .tempdir_in(parent)
        .map_err(
            |_| "Cannot stage an update beside the app. Install it manually in a writable folder.",
        )?;
    let lockfile = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(parent.join(".openradview-update.lock"))
        .map_err(|_| "Cannot lock the app folder for an update.")?;
    lockfile
        .try_lock()
        .map_err(|_| "Another OpenRadView instance is installing an update.")?;
    let archive = staging.path().join(&manifest.archive);
    let mut percent = 0;
    network::download(version, manifest, &archive, |bytes| {
        let next = (bytes * 100 / manifest.bytes).min(100);
        if next != percent {
            percent = next;
            lock(state).downloaded = bytes;
            notify();
        }
    })?;
    change(state, notify, Status::Verifying, "");
    let staged = unpack_bundle(&archive, &staging.path().join("unpacked"), manifest)?;
    install::validate_stage(&staged, &current, manifest)?;
    install::current_trust(&current, Some(version), || lifecycle.closing())?;
    let _commit = lifecycle.begin_commit()?;
    change(state, notify, Status::Installing, "");
    install::swap(&staged, &current, exe)
}

fn unpack_bundle(
    archive: &std::path::Path,
    extracted: &std::path::Path,
    manifest: &Manifest,
) -> Result<PathBuf> {
    std::fs::create_dir(extracted).map_err(|_| "Cannot prepare the update directory.")?;
    archive::unpack(archive, extracted)?;
    let contents: Vec<_> = std::fs::read_dir(extracted)
        .map_err(|_| "Cannot inspect the staged update.")?
        .collect();
    if contents.len() != 1
        || contents[0]
            .as_ref()
            .map_err(|_| "Cannot inspect the staged update.")?
            .file_name()
            != manifest.bundle.as_str()
    {
        return Err("The update archive has an unexpected layout.");
    }
    Ok(extracted.join(&manifest.bundle))
}
fn worker(
    state: &Mutex<State>,
    notify: &dyn Fn(),
    prefs_path: &std::path::Path,
    exe: &std::path::Path,
    version: &str,
    receive: mpsc::Receiver<Task>,
    lifecycle: &lifecycle::Lifecycle,
) {
    let cleanup = lock(state).prefs.cleanup.clone();
    if let Some(cleanup) = cleanup.filter(|c| c.installed_version == version)
        && let Ok(current) = install::root(exe)
        && install::cleanup(&cleanup.backup, &current, || lifecycle.closing()).is_ok()
    {
        let mut state = lock(state);
        state.prefs.cleanup = None;
        let _ = preferences::save(prefs_path, &state.prefs);
    }
    let mut manual_check = None;
    loop {
        let automatic = {
            let s = lock(state);
            automatic_due(&s.prefs, s.status, now(), s.package_managed)
        };
        let task = if automatic {
            Task::Check
        } else {
            match receive.recv_timeout(Duration::from_secs(60)) {
                Ok(task) => task,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            }
        };
        match task {
            Task::Stop => break,
            Task::Wake => continue,
            Task::Check => {
                if lock(state).status == Status::ReadyToRestart {
                    continue;
                }
                if manual_check
                    .is_some_and(|t: std::time::Instant| t.elapsed() < Duration::from_secs(60))
                {
                    change(
                        state,
                        notify,
                        Status::Failed,
                        "Please wait a minute between update checks.",
                    );
                    continue;
                }
                manual_check = Some(std::time::Instant::now());
                {
                    let mut state = lock(state);
                    state.prefs.last_check = now();
                    if preferences::save(prefs_path, &state.prefs).is_err() && automatic {
                        state.prefs.enabled = false;
                        state.prefs.chosen = false;
                        state.status = Status::Failed;
                        state.detail = "Cannot save update preferences. Automatic checks are off; no background request was sent.";
                        notify();
                        continue;
                    }
                    state.manifest = None;
                }
                change(state, notify, Status::Checking, "");
                match check(version) {
                    Ok(Some(manifest)) => {
                        let readiness = install::root(exe).and_then(|root| {
                            install::current_trust(&root, Some(version), || lifecycle.closing())
                        });
                        {
                            let mut s = lock(state);
                            s.manifest = Some(manifest);
                            s.installable = readiness.is_ok();
                        }
                        change(
                            state,
                            notify,
                            Status::Available,
                            readiness.err().unwrap_or(""),
                        );
                    }
                    Ok(None) => change(state, notify, Status::Current, ""),
                    Err(message) => change(state, notify, Status::Failed, message),
                }
            }
            Task::Install => {
                let manifest = {
                    let mut s = lock(state);
                    if s.status != Status::Available {
                        continue;
                    }
                    s.downloaded = 0;
                    s.manifest.clone()
                };
                let Some(manifest) = manifest else {
                    continue;
                };
                change(state, notify, Status::Downloading, "");
                match perform_install(state, notify, exe, version, &manifest, lifecycle) {
                    Ok(backup) => {
                        {
                            let mut s = lock(state);
                            s.prefs.cleanup = Some(Cleanup {
                                backup,
                                installed_version: manifest.version.clone(),
                            });
                            let _ = preferences::save(prefs_path, &s.prefs);
                        }
                        change(state, notify, Status::ReadyToRestart, "");
                    }
                    Err(message) => change(state, notify, Status::Failed, message),
                }
            }
        }
    }
}

/// Public-key-only tooling for an owner-prepared release envelope. This does
/// not download, install or authorize anything, and never accesses credentials.
pub fn verify_release_envelope(
    bytes: &[u8],
    target: &str,
) -> std::result::Result<UpdateInfo, &'static str> {
    let manifest = protocol::verify_envelope(bytes, &protocol::public_key()?, target)?;
    Ok(UpdateInfo {
        version: manifest.version,
        bytes: manifest.bytes,
    })
}

fn package_install(executable: &std::path::Path) -> bool {
    executable.parent().is_some_and(|root| {
        std::fs::symlink_metadata(root.join(".openradview-no-self-update")).is_ok()
    })
}

fn automatic_due(prefs: &Preferences, status: Status, time: u64, package_managed: bool) -> bool {
    !package_managed
        && prefs.chosen
        && prefs.enabled
        && !status.busy()
        && status != Status::ReadyToRestart
        && time.saturating_sub(prefs.last_check) >= 86400
}
