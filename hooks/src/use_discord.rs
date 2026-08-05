//! Desired and observed Discord Rich Presence connection state.

use std::sync::{Arc, RwLock};

use dioxus::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscordConnection {
    Off,
    Connecting,
    Connected,
    Waiting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiscordRuntime {
    pub enabled: bool,
    pub revision: u64,
    pub connection: DiscordConnection,
}

#[derive(Clone)]
pub struct UseDiscordPresence(Arc<RwLock<DiscordRuntime>>);

impl UseDiscordPresence {
    pub(crate) fn new(enabled: bool) -> Self {
        Self(Arc::new(RwLock::new(DiscordRuntime {
            enabled,
            revision: 0,
            connection: if enabled {
                DiscordConnection::Connecting
            } else {
                DiscordConnection::Off
            },
        })))
    }

    pub fn runtime(&self) -> DiscordRuntime {
        *self.0.read().expect("Discord state lock poisoned")
    }

    pub fn connect(&self) {
        let mut state = self.0.write().expect("Discord state lock poisoned");
        state.enabled = true;
        state.revision = state.revision.wrapping_add(1);
        state.connection = DiscordConnection::Connecting;
    }

    pub fn disconnect(&self) {
        let mut state = self.0.write().expect("Discord state lock poisoned");
        state.enabled = false;
        state.revision = state.revision.wrapping_add(1);
        state.connection = DiscordConnection::Off;
    }

    pub fn set_connection(&self, revision: u64, connection: DiscordConnection) {
        let mut state = self.0.write().expect("Discord state lock poisoned");
        if state.revision == revision {
            state.connection = connection;
        }
    }
}

pub(crate) fn install_discord_presence(enabled: bool) {
    let control = UseDiscordPresence::new(enabled);
    use_context_provider(move || control);
}

pub fn use_discord_presence() -> UseDiscordPresence {
    use_context::<UseDiscordPresence>()
}

#[cfg(test)]
mod tests {
    use super::{DiscordConnection, UseDiscordPresence};

    #[test]
    fn connect_and_disconnect_advance_the_revision() {
        let control = UseDiscordPresence::new(false);
        let first = control.runtime();

        control.connect();
        let connecting = control.runtime();
        assert!(connecting.enabled);
        assert_eq!(connecting.connection, DiscordConnection::Connecting);
        assert!(connecting.revision > first.revision);

        control.disconnect();
        let off = control.runtime();
        assert!(!off.enabled);
        assert_eq!(off.connection, DiscordConnection::Off);
        assert!(off.revision > connecting.revision);
    }

    #[test]
    fn stale_bridge_status_cannot_overwrite_disconnect() {
        let control = UseDiscordPresence::new(true);
        let old_revision = control.runtime().revision;

        control.disconnect();
        control.set_connection(old_revision, DiscordConnection::Connected);

        assert_eq!(control.runtime().connection, DiscordConnection::Off);
    }
}
