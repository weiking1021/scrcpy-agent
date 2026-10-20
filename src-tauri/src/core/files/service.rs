//! Files and a device: sending them into its folder, and opening them there
//! with one of its apps.

use super::file_types::{FileTypeError, FileTypes};
use super::openers::{self, OpenType};
use super::transfer;
use crate::core::adb::api::AdbApi;
use crate::core::adb::select::ConfiguredAdb;
use crate::core::context::Ctx;
use crate::core::error::CoreError;
use crate::core::log::{LogCode, LogLevel};
use crate::core::model::{Event, OpenSupport, OpenedUpdate, SendOutcome, TransferPhase, TransferUpdate};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub struct FileService {
    ctx: Arc<Ctx>,
    /// The kinds of files that add-ons have named.
    file_types: Mutex<FileTypes>,
    /// One lock for each device, so files sent to it one after the other cannot
    /// choose the same name.
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    next_transfer: AtomicU64,
}

impl FileService {
    pub(crate) fn new(ctx: Arc<Ctx>) -> Arc<Self> {
        Arc::new(FileService {
            ctx,
            file_types: Mutex::new(FileTypes::new()),
            locks: Mutex::new(HashMap::new()),
            next_transfer: AtomicU64::new(1),
        })
    }

    /// Names the type of the files with an extension, for opening them on a device
    /// when the system has no type for them.
    pub fn register_file_type(&self, addon: &str, extension: &str, mime: &str) -> Result<(), FileTypeError> {
        self.file_types.lock().unwrap().register(addon, extension, mime)
    }

    /// Takes back the file types an add-on named.
    pub fn remove_addon(&self, addon: &str) {
        self.file_types.lock().unwrap().remove_addon(addon);
    }

    /// Sends files to a device, into the folder of the setting, one after the
    /// other. A file whose name is taken there gets a number. One file that
    /// cannot be sent does not stop the others: each has its outcome.
    pub async fn send_files(&self, device_id: &str, files: Vec<PathBuf>) -> Result<Vec<SendOutcome>, CoreError> {
        let (serial, _) = self.ctx.online_device(device_id)?;
        let dir = transfer::RemoteDir::parse(&self.ctx.settings().send_directory).ok_or(CoreError::InvalidDirectory)?;
        let adb = self.ctx.adb()?;
        // One batch at a time for a device: two could choose the same name.
        let lock = self.locks.lock().unwrap().entry(device_id.to_string()).or_default().clone();
        let _turn = lock.lock().await;

        let shell = |command: String| {
            let (adb, serial) = (&adb, &serial);
            async move {
                match adb.shell(serial, &command).await {
                    Ok(out) if out.exit == 0 => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
                    Ok(out) => Err(CoreError::CommandFailed(out.exit)),
                    Err(e) => Err(CoreError::from(e)),
                }
            }
        };
        let quoted = transfer::shell_quote(dir.as_str());
        // The root has the empty path: it exists, and `mkdir -p ''` would not.
        if !dir.as_str().is_empty() {
            shell(format!("mkdir -p {quoted}")).await?;
        }
        let listed = if dir.as_str().is_empty() { "/" } else { dir.as_str() };
        let mut existing = transfer::parse_listing(&shell(format!("ls -1 {}", transfer::shell_quote(listed))).await?);

        let mut outcomes = Vec::new();
        for local in files {
            let file = local.display().to_string();
            let outcome = self.send_one(&adb, &serial, device_id, &dir, &mut existing, &local).await;
            outcomes.push(match outcome {
                Ok(remote) => SendOutcome { file, remote: Some(remote), error: None },
                Err(error) => SendOutcome { file, remote: None, error: Some(error) },
            });
        }
        Ok(outcomes)
    }

    /// Sends one file; `existing` are the names in the folder, and the new one is added.
    async fn send_one(
        &self,
        adb: &ConfiguredAdb,
        serial: &str,
        device_id: &str,
        dir: &transfer::RemoteDir,
        existing: &mut Vec<String>,
        local: &Path,
    ) -> Result<String, CoreError> {
        let id = self.next_transfer.fetch_add(1, Ordering::Relaxed);
        let update = |name: &str, sent: u64, total: u64, phase: TransferPhase, error: Option<CoreError>| {
            self.ctx.send(Event::Transfer(Box::new(TransferUpdate {
                id,
                device: device_id.to_string(),
                name: name.to_string(),
                sent,
                total,
                phase,
                error,
            })));
        };
        let log = &self.ctx.logger;
        let name = transfer::file_name_of(local);
        let shown = name.clone().unwrap_or_else(|| local.display().to_string());
        let fail = |error: CoreError| {
            log.log(LogLevel::Warn, LogCode::Transfer, format!("{shown} was not sent to {device_id}: {error}"));
            update(&shown, 0, 0, TransferPhase::Failed, Some(error.clone()));
            Err(error)
        };
        let Some(name) = name else { return fail(CoreError::BadFileName) };
        let total = match std::fs::metadata(local) {
            Ok(meta) if meta.is_file() => meta.len(),
            _ => return fail(CoreError::NotAFile),
        };
        let unique = transfer::unique_name(existing, &name);
        let remote = dir.join(&unique);
        update(&unique, 0, total, TransferPhase::Sending, None);

        // Told after every piece; passed on when it moved by a hundredth of the file
        // (and at least a megabyte), so a big file does not flood the UI.
        let step = (total / 100).max(1 << 20);
        let last = AtomicU64::new(0);
        let progress = |sent: u64| {
            if sent >= last.load(Ordering::Relaxed) + step && sent < total {
                last.store(sent, Ordering::Relaxed);
                update(&unique, sent, total, TransferPhase::Sending, None);
            }
        };
        match adb.push(serial, local, &remote, &progress).await {
            Ok(()) => {
                existing.push(unique.clone());
                update(&unique, total, total, TransferPhase::Done, None);
                log.log(LogLevel::Info, LogCode::Transfer, format!("{unique} ({total} bytes) sent to {device_id}"));
                Ok(remote)
            }
            Err(e) => fail(CoreError::from(e)),
        }
    }

    /// Whether the device has an app that can open this file.
    pub async fn open_support(&self, device_id: &str, file: &Path) -> Result<OpenSupport, CoreError> {
        let (serial, _) = self.ctx.online_device(device_id)?;
        let mime = self.open_type_of(file);
        let uri = self.planned_uri(file)?;
        let adb = self.ctx.adb()?;
        let Ok(handlers) = self.handlers_of(&adb, &serial, &uri, &mime).await else { return Ok(OpenSupport::Unreachable) };
        if handlers.is_empty() {
            return Ok(OpenSupport::NoApp);
        }
        // Whether the user made one of them the default is a second question.
        let is_default = match adb.shell(&serial, &openers::resolve_command(&uri, &mime)).await {
            Ok(out) if out.exit == 0 => {
                openers::parse_resolved(&String::from_utf8_lossy(&out.stdout)).is_some_and(|r| r.is_default)
            }
            _ => false,
        };
        Ok(OpenSupport::Apps { count: handlers.len(), is_default, type_known: matches!(mime, OpenType::Known(_)) })
    }

    /// The address the file will have on the device when it is sent, as far as it can be
    /// known before: the folder of the setting and the name of the file. (A name that is
    /// taken gets a number when the file is sent; the address that the file is started with
    /// then differs from this one by that.) The questions to the device are asked with it:
    /// they must have the shape of the start (see `openers::query_command`).
    fn planned_uri(&self, file: &Path) -> Result<String, CoreError> {
        let directory = self.ctx.settings().send_directory;
        let dir = transfer::RemoteDir::parse(&directory).ok_or(CoreError::InvalidDirectory)?;
        let name = transfer::file_name_of(file).ok_or(CoreError::BadFileName)?;
        Ok(openers::file_uri(&dir.join(&name)))
    }

    /// The type to open a file with: the system's, an add-on's, the table's, else any.
    fn open_type_of(&self, file: &Path) -> OpenType {
        let types = self.file_types.lock().unwrap();
        openers::open_type_of_file(file, &types)
    }

    /// The apps of the device that can open this kind of file.
    async fn handlers_of(
        &self,
        adb: &ConfiguredAdb,
        serial: &str,
        uri: &str,
        mime: &OpenType,
    ) -> Result<Vec<openers::Component>, CoreError> {
        let out = adb.shell(serial, &openers::query_command(uri, mime)).await?;
        if out.exit != 0 {
            return Err(CoreError::CommandFailed(out.exit));
        }
        Ok(openers::parse_handlers(&String::from_utf8_lossy(&out.stdout)))
    }

    /// Sends the file to the device and has the device open it with an app. The
    /// kind of the file and the apps are checked before anything is sent.
    pub async fn open_on_device(&self, device_id: &str, file: &Path) -> Result<String, CoreError> {
        let result = self.open_inner(device_id, file).await;
        let (remote, error) = match &result {
            Ok(remote) => (Some(remote.clone()), None),
            Err(error) => (None, Some(error.clone())),
        };
        self.ctx.send(Event::Opened(Box::new(OpenedUpdate {
            device: device_id.to_string(),
            file: file.display().to_string(),
            remote,
            error,
        })));
        result
    }

    async fn open_inner(&self, device_id: &str, file: &Path) -> Result<String, CoreError> {
        let (serial, _) = self.ctx.online_device(device_id)?;
        let mime = self.open_type_of(file);
        let adb = self.ctx.adb()?;
        if self.handlers_of(&adb, &serial, &self.planned_uri(file)?, &mime).await?.is_empty() {
            return Err(CoreError::NoApp);
        }
        let outcome = self.send_files(device_id, vec![file.to_path_buf()]).await?.into_iter().next();
        let remote = match outcome {
            Some(SendOutcome { remote: Some(remote), .. }) => remote,
            Some(SendOutcome { error: Some(error), .. }) => return Err(error),
            _ => return Err(CoreError::NotAFile),
        };
        let start = openers::start_command(&openers::file_uri(&remote), &mime);
        let out = adb.shell(&serial, &start).await?;
        if out.exit != 0 {
            self.ctx.logger.log(LogLevel::Warn, LogCode::Transfer, format!("{remote} could not be opened on {device_id}"));
            return Err(CoreError::CommandFailed(out.exit));
        }
        self.ctx.logger.log(LogLevel::Info, LogCode::Transfer, format!("{remote} opened on {device_id}"));
        Ok(remote)
    }
}
