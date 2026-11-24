//! Text payloads from a drag that is not a file path.
//!
//! winit's public drop event is `DroppedFile(PathBuf)`. Browser drags are
//! often a URL (`text/uri-list`, `UniformResourceLocator`) with no file.
//! Platform backends push those strings here; the application drains them
//! on the same frame as the drop. The queue is only touched on the event
//! loop thread and the application's main thread.

use std::sync::Mutex;

static DROPPED: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub(crate) fn push(text: String) {
    if text.is_empty() {
        return;
    }
    if let Ok(mut queue) = DROPPED.lock() {
        queue.push(text);
    }
}

/// Take every URL (or uri-list blob) dropped since the last call.
pub fn take_dropped_text() -> Vec<String> {
    DROPPED
        .lock()
        .map(|mut queue| std::mem::take(&mut *queue))
        .unwrap_or_default()
}
