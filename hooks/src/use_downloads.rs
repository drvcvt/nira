//! Global background-operation status rendered by the app-shell toast.

use dioxus::prelude::*;

#[derive(Clone, Copy)]
pub struct UseDownloads {
    pub status: Signal<Option<String>>,
    pub busy: Signal<bool>,
    pub failed: Signal<bool>,
}

impl UseDownloads {
    pub fn start(&self, message: impl Into<String>) {
        self.set(message, true, false);
    }

    pub fn finish(&self, message: impl Into<String>) {
        self.set(message, false, false);
    }

    pub fn fail(&self, message: impl Into<String>) {
        self.set(message, false, true);
    }

    pub fn dismiss(&self) {
        let mut status = self.status;
        status.set(None);
    }

    fn set(&self, message: impl Into<String>, busy_value: bool, failed_value: bool) {
        let mut status = self.status;
        let mut busy = self.busy;
        let mut failed = self.failed;
        status.set(Some(message.into()));
        busy.set(busy_value);
        failed.set(failed_value);
    }
}

pub fn install_downloads() {
    let state = use_hook(|| UseDownloads {
        status: Signal::new_in_scope(None, ScopeId::ROOT),
        busy: Signal::new_in_scope(false, ScopeId::ROOT),
        failed: Signal::new_in_scope(false, ScopeId::ROOT),
    });
    use_context_provider(move || state);
}

pub fn use_downloads() -> UseDownloads {
    use_context::<UseDownloads>()
}
