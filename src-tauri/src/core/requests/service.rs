//! The requests that come from the file manager's menu (send these files,
//! open this file on a device): carrying them out, asking the user which
//! device when that is not known, and what the small window of a request shows.

use super::request::{ItemState, RequestView};
use super::shell_request::ShellAction;
use crate::core::context::Ctx;
use crate::core::error::CoreError;
use crate::core::files::service::FileService;
use crate::core::log::{LogCode, LogLevel};
use crate::core::model::{ChoiceInfo, Event, SendOutcome};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What `RequestService::handle_shell` did with a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellStart {
    /// It is being carried out on the device.
    Running,
    /// The user has to choose a device first (`RequestService::pending_choice`).
    NeedsChoice,
}

/// A request that waits for the user to choose a device.
struct PendingChoice {
    /// The request (see `RequestView::id`) this waits for.
    id: u64,
    action: ShellAction,
    files: Vec<PathBuf>,
    no_default: bool,
}

pub struct RequestService {
    ctx: Arc<Ctx>,
    files: Arc<FileService>,
    /// The request from the file manager's menu that waits for a device to be chosen.
    choice: Mutex<Option<PendingChoice>>,
    /// The request the small window shows, and the number of the next one.
    request: Mutex<Option<RequestView>>,
    next_request: AtomicU64,
}

impl RequestService {
    pub(crate) fn new(ctx: Arc<Ctx>, files: Arc<FileService>) -> Arc<Self> {
        Arc::new(RequestService {
            ctx,
            files,
            choice: Mutex::new(None),
            request: Mutex::new(None),
            next_request: AtomicU64::new(1),
        })
    }

    /// Carries out a request of the file manager's menu: on the default device if the
    /// action says so and there is one that is ready, else the user is asked.
    pub async fn handle_shell(self: &Arc<Self>, action: ShellAction, files: Vec<PathBuf>) -> ShellStart {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        if action.uses_default() {
            let default = self.ctx.settings().default_device;
            if let Some(device) = default.filter(|d| self.ctx.online_device(d).is_ok()) {
                let mut view = RequestView::new(id, action, &files, false, self.close_after());
                view.choose(&device);
                self.show(Some(view));
                self.run_shell(id, action, device, files);
                return ShellStart::Running;
            }
            self.set_choice(id, action, files, true);
        } else {
            self.set_choice(id, action, files, false);
        }
        ShellStart::NeedsChoice
    }

    /// How long the small window of a request stays after everything went well (the setting).
    fn close_after(&self) -> Duration {
        Duration::from_secs(u64::from(self.ctx.settings().request_close_seconds.seconds()))
    }

    /// Makes `view` what the small window shows (or nothing), and tells the window.
    fn show(&self, view: Option<RequestView>) {
        *self.request.lock().unwrap() = view.clone();
        self.ctx.send(Event::Request(view.map(Box::new)));
    }

    /// Changes the view of request `id`, if it is still the one that is shown (a newer
    /// request, or a window that was closed, is not touched), and tells the window.
    fn update(&self, id: u64, change: impl FnOnce(&mut RequestView)) {
        let changed = {
            let mut current = self.request.lock().unwrap();
            match current.as_mut().filter(|v| v.id == id) {
                Some(view) => {
                    change(view);
                    Some(view.clone())
                }
                None => None,
            }
        };
        if let Some(view) = changed {
            self.ctx.send(Event::Request(Some(Box::new(view))));
        }
    }

    /// Makes a request wait for the user to choose a device (a newer one takes the
    /// place of one that waited).
    fn set_choice(&self, id: u64, action: ShellAction, files: Vec<PathBuf>, no_default: bool) {
        self.ctx.logger.log(
            LogLevel::Info,
            LogCode::Transfer,
            format!("{} file(s) wait for a device to be chosen ({})", files.len(), action.word()),
        );
        self.show(Some(RequestView::new(id, action, &files, no_default, self.close_after())));
        *self.choice.lock().unwrap() = Some(PendingChoice { id, action, files, no_default });
    }

    /// Does what the action says with the files, on the device, without waiting
    /// for it to end. How each file goes is put into the view of request `id`.
    fn run_shell(self: &Arc<Self>, id: u64, action: ShellAction, device: String, files: Vec<PathBuf>) {
        let me = self.clone();
        tokio::spawn(async move {
            let failed = |error: CoreError| ItemState::Failed { error };
            if action.opens() {
                for (index, file) in files.iter().enumerate() {
                    me.update(id, |v| v.set_item(index, ItemState::Working));
                    // The end of each is also announced as an event (`opened`).
                    let state = match me.files.open_on_device(&device, file).await {
                        Ok(_) => ItemState::Done,
                        Err(error) => failed(error),
                    };
                    me.update(id, |v| v.set_item(index, state));
                }
            } else {
                let count = files.len();
                me.update(id, |v| (0..count).for_each(|i| v.set_item(i, ItemState::Working)));
                let outcomes = me.files.send_files(&device, files).await;
                me.update(id, |v| {
                    for index in 0..count {
                        // One answer for each file; a batch that could not start failed as a whole.
                        let state = match &outcomes {
                            Ok(all) => match all.get(index) {
                                Some(SendOutcome { error: Some(error), .. }) => failed(error.clone()),
                                Some(_) => ItemState::Done,
                                None => failed(CoreError::NotAFile),
                            },
                            Err(error) => failed(error.clone()),
                        };
                        v.set_item(index, state);
                    }
                });
            }
        });
    }

    /// The request the small window shows now.
    pub fn current_request(&self) -> Option<RequestView> {
        self.request.lock().unwrap().clone()
    }

    /// The window of request `id` is closed (it closed itself, or the user did); a
    /// request that has taken its place is not touched.
    pub fn dismiss_request(&self, id: u64) {
        {
            let mut current = self.request.lock().unwrap();
            if current.as_ref().is_none_or(|view| view.id != id) { return; }
            *current = None;
        }
        // A close of the window also drops its device question. A newer
        // request must not be cleared between checking the id and dismissing it.
        {
            let mut choice = self.choice.lock().unwrap();
            if choice.as_ref().is_some_and(|pending| pending.id == id) { *choice = None; }
        }
        self.ctx.send(Event::Request(None));
    }

    /// The request that waits for a device to be chosen.
    pub fn pending_choice(&self) -> Option<ChoiceInfo> {
        self.choice.lock().unwrap().as_ref().map(|c| ChoiceInfo {
            action: c.action,
            files: c.files.iter().map(|f| f.display().to_string()).collect(),
            no_default: c.no_default,
        })
    }

    /// Carries out the request that waits, on this device. A device that is not
    /// there leaves the request waiting.
    pub async fn resolve_choice(self: &Arc<Self>, device_id: &str) -> Result<(), CoreError> {
        self.ctx.online_device(device_id)?;
        let pending = self.choice.lock().unwrap().take().ok_or(CoreError::NotAvailable)?;
        self.update(pending.id, |v| v.choose(device_id));
        self.run_shell(pending.id, pending.action, device_id.to_string(), pending.files);
        Ok(())
    }

    /// Drops the request that waits.
    pub fn cancel_choice(&self) {
        let pending = self.choice.lock().unwrap().take();
        if let Some(pending) = pending {
            self.dismiss_request(pending.id);
        }
    }
}
