//! What the small window of a request from a file manager shows: the files, the
//! device (asked for when there is none yet) and how each file is doing. The state
//! and what follows from it (when the window may close itself) are here, so they are
//! the same on every system and tested; the window only draws it and sends the
//! user's choices back.

use crate::core::error::CoreError;
use super::shell_request::ShellAction;
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

/// How long the window stays after everything went well, for the user to see it (the default of the setting).
pub const DONE_CLOSE_AFTER: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// There is no device yet: the user is asked.
    Choosing,
    /// At least one file is waiting or being done.
    Working,
    /// Every file was done.
    Done,
    /// Every file ended, and at least one did not go well. The window stays.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ItemState {
    Waiting,
    Working,
    Done,
    Failed { error: CoreError },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RequestItem {
    /// The file as it was given.
    pub file: String,
    #[serde(flatten)]
    pub state: ItemState,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RequestView {
    /// Tells this request from an older one whose work is still going on.
    pub id: u64,
    pub action: ShellAction,
    /// The device chosen (or the default one); `None` while it is asked for.
    pub device: Option<String>,
    /// The user is asked because there is no default device that is ready.
    pub no_default: bool,
    pub items: Vec<RequestItem>,
    pub phase: Phase,
    /// The window closes itself after this many milliseconds, if it should.
    pub close_after_ms: Option<u64>,
    /// How long the window stays once everything went well (the setting); not sent.
    #[serde(skip)]
    close_after: Duration,
}

impl RequestView {
    /// `close_after` is how long the window stays once everything went well.
    pub fn new(id: u64, action: ShellAction, files: &[PathBuf], no_default: bool, close_after: Duration) -> Self {
        let mut view = RequestView {
            id,
            action,
            device: None,
            no_default,
            items: files.iter().map(|f| RequestItem { file: f.display().to_string(), state: ItemState::Waiting }).collect(),
            phase: Phase::Choosing,
            close_after_ms: None,
            close_after,
        };
        view.update();
        view
    }

    /// The request goes to this device.
    pub fn choose(&mut self, device: &str) {
        self.device = Some(device.to_string());
        self.update();
    }

    /// Sets how a file is doing; an index that is not a file is ignored.
    pub fn set_item(&mut self, index: usize, state: ItemState) {
        if let Some(item) = self.items.get_mut(index) {
            item.state = state;
            self.update();
        }
    }

    /// The phase, and what follows from it, from the device and the files.
    fn update(&mut self) {
        let going = |s: &ItemState| matches!(s, ItemState::Waiting | ItemState::Working);
        self.phase = if self.device.is_none() {
            Phase::Choosing
        } else if self.items.iter().any(|i| going(&i.state)) {
            Phase::Working
        } else if self.items.iter().any(|i| matches!(i.state, ItemState::Failed { .. })) {
            Phase::Failed
        } else {
            Phase::Done
        };
        self.close_after_ms = (self.phase == Phase::Done).then_some(self.close_after.as_millis() as u64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    fn view(names: &[&str]) -> RequestView {
        RequestView::new(7, ShellAction::OpenChoose, &files(names), false, DONE_CLOSE_AFTER)
    }

    #[test]
    fn the_time_the_window_stays_after_success_is_the_one_it_was_given_and_zero_closes_it_at_once() {
        for (seconds, ms) in [(0u64, 0u64), (1, 1000), (5, 5000), (60, 60000)] {
            let mut v = RequestView::new(1, ShellAction::OpenChoose, &files(&["a"]), false, Duration::from_secs(seconds));
            v.choose("d");
            assert_eq!(v.close_after_ms, None, "still working");
            v.set_item(0, ItemState::Done);
            assert_eq!(v.close_after_ms, Some(ms), "{seconds} s");
            // A failure is never closed by itself, whatever the time is.
            let mut failed = RequestView::new(1, ShellAction::OpenChoose, &files(&["a"]), false, Duration::from_secs(seconds));
            failed.choose("d");
            failed.set_item(0, ItemState::Failed { error: CoreError::NotAFile });
            assert_eq!(failed.close_after_ms, None, "{seconds} s");
        }
    }

    fn failed() -> ItemState {
        ItemState::Failed { error: CoreError::NotAFile }
    }

    #[test]
    fn a_new_request_has_no_device_and_asks_for_one_with_every_file_waiting() {
        let v = view(&["/tmp/a.txt", "/tmp/b c.log"]);
        assert_eq!(v.id, 7);
        assert_eq!(v.device, None);
        assert_eq!(v.phase, Phase::Choosing);
        assert_eq!(v.close_after_ms, None);
        assert_eq!(
            v.items,
            [
                RequestItem { file: "/tmp/a.txt".into(), state: ItemState::Waiting },
                RequestItem { file: "/tmp/b c.log".into(), state: ItemState::Waiting },
            ]
        );
    }

    #[test]
    fn choosing_a_device_starts_the_work_and_each_file_moves_on_by_itself() {
        let mut v = view(&["a", "b"]);
        v.choose("device-1");
        assert_eq!(v.device.as_deref(), Some("device-1"));
        assert_eq!(v.phase, Phase::Working);
        assert_eq!(v.close_after_ms, None);
        v.set_item(0, ItemState::Working);
        assert_eq!(v.phase, Phase::Working);
        v.set_item(0, ItemState::Done);
        // The second is still waiting.
        assert_eq!(v.phase, Phase::Working);
        assert_eq!(v.items[0].state, ItemState::Done);
        assert_eq!(v.items[1].state, ItemState::Waiting);
    }

    #[test]
    fn when_every_file_is_done_the_window_may_close_itself_after_a_moment() {
        let mut v = view(&["a", "b"]);
        v.choose("d");
        v.set_item(0, ItemState::Done);
        v.set_item(1, ItemState::Done);
        assert_eq!(v.phase, Phase::Done);
        assert_eq!(v.close_after_ms, Some(DONE_CLOSE_AFTER.as_millis() as u64));
    }

    #[test]
    fn a_file_that_failed_keeps_the_window_open_once_the_others_have_ended_too() {
        let mut v = view(&["a", "b", "c"]);
        v.choose("d");
        v.set_item(0, failed());
        // Another is still going: it is working, not failed yet.
        assert_eq!(v.phase, Phase::Working);
        v.set_item(1, ItemState::Done);
        v.set_item(2, ItemState::Working);
        assert_eq!(v.phase, Phase::Working);
        v.set_item(2, ItemState::Done);
        assert_eq!(v.phase, Phase::Failed);
        assert_eq!(v.close_after_ms, None);
        assert_eq!(v.items[0].state, failed());
    }

    #[test]
    fn a_request_that_goes_to_the_default_device_needs_no_question() {
        // The device is set by the same call, so the window never asks.
        let mut v = RequestView::new(1, ShellAction::SendDefault, &files(&["a"]), false, DONE_CLOSE_AFTER);
        assert_eq!(v.phase, Phase::Choosing, "until it has a device");
        v.choose("the-default");
        assert_eq!(v.device.as_deref(), Some("the-default"));
        assert_eq!(v.phase, Phase::Working);
    }

    #[test]
    fn an_index_that_is_not_a_file_changes_nothing() {
        let mut v = view(&["a"]);
        v.choose("d");
        v.set_item(0, ItemState::Working);
        assert_eq!(v.items[0].state, ItemState::Working, "an index that is a file does change it");
        let before = v.clone();
        v.set_item(1, ItemState::Done);
        v.set_item(usize::MAX, ItemState::Done);
        assert_eq!(v, before);
    }

    #[test]
    fn a_request_without_files_is_done_as_soon_as_it_has_a_device() {
        let mut v = view(&[]);
        assert_eq!(v.phase, Phase::Choosing);
        v.choose("d");
        assert_eq!(v.phase, Phase::Done);
    }

    #[test]
    fn the_view_is_sent_to_the_window_with_the_state_of_each_file_written_out() {
        let mut v = view(&["/tmp/a.txt", "/tmp/b.txt"]);
        v.choose("d");
        v.set_item(0, ItemState::Done);
        v.set_item(1, failed());
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["phase"], "failed");
        assert_eq!(json["action"], "open_choose");
        assert_eq!(json["device"], "d");
        assert_eq!(json["close_after_ms"], serde_json::Value::Null);
        assert_eq!(json["items"][0], serde_json::json!({ "file": "/tmp/a.txt", "state": "done" }));
        assert_eq!(
            json["items"][1],
            serde_json::json!({ "file": "/tmp/b.txt", "state": "failed", "error": { "code": "not_a_file" } })
        );
    }
}
