use crate::managers::PlayerManager;
use parking_lot::Mutex as SyncMutex;
use std::sync::atomic::AtomicU32;

pub struct ServerState {
    pub player_count: AtomicU32,
    // make player_manager SyncRwLock if there will be read-only operations
    // for now everything requires write access so it's a waste
    pub player_manager: SyncMutex<PlayerManager>,
}

impl ServerState {
    pub fn new() -> Self {
        Self {
            player_count: AtomicU32::new(0u32),
            player_manager: SyncMutex::new(PlayerManager::new()),
        }
    }
}

impl Default for ServerState {
    fn default() -> Self {
        Self::new()
    }
}
