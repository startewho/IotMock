//! Protocol abstraction layer.
//!
//! Every simulation server (Modbus TCP today, S7 / OPC-UA / MQTT tomorrow)
//! implements the [`Protocol`] trait. The UI only knows about this trait, so
//! adding a new protocol is:
//!
//! 1. implement [`Protocol`] for your server,
//! 2. push a `Box<dyn Protocol>` into the protocol list in `AppView::new`,
//! 3. done — a control card shows up automatically in the sidebar.
//!
//! All protocols share the same [`ProtocolContext`] (register store + stats),
//! so data written by one protocol is immediately visible to the others and
//! to the UI.

pub mod modbus;

use std::sync::{
    Arc,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

use crate::model::SharedStore;

/// Aggregate runtime statistics for one protocol server.
#[derive(Debug, Default)]
pub struct ServerStats {
    pub current_clients: AtomicUsize,
    pub peak_clients: AtomicUsize,
    pub total_requests: AtomicU64,
    pub cells_written: AtomicU64,
    pub error_responses: AtomicU64,
}

impl ServerStats {
    /// Record a client connection.
    pub fn client_connected(&self) {
        let cur = self.current_clients.fetch_add(1, Ordering::Relaxed) + 1;
        let mut peak = self.peak_clients.load(Ordering::Relaxed);
        while cur > peak {
            match self
                .peak_clients
                .compare_exchange_weak(peak, cur, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(actual) => peak = actual,
            }
        }
    }

    pub fn client_disconnected(&self) {
        self.current_clients.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn requests(&self, n: u64) {
        self.total_requests.fetch_add(n, Ordering::Relaxed);
    }

    pub fn cells_written(&self, n: u64) {
        self.cells_written.fetch_add(n, Ordering::Relaxed);
    }

    pub fn errors(&self, n: u64) {
        self.error_responses.fetch_add(n, Ordering::Relaxed);
    }
}

/// Server lifecycle state, shown in the UI as a badge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerState {
    Stopped,
    Starting,
    Running,
    Stopping,
    /// A fatal error (for example, the port is already in use).
    Error(String),
}

impl ServerState {
    pub fn label(&self) -> &'static str {
        match self {
            ServerState::Stopped => "已停止",
            ServerState::Starting => "启动中",
            ServerState::Running => "运行中",
            ServerState::Stopping => "停止中",
            ServerState::Error(_) => "错误",
        }
    }
}

/// Shared context handed to every protocol server at `start()` time.
#[derive(Clone)]
pub struct ProtocolContext {
    /// The single register store shared by all protocols and the UI.
    pub store: SharedStore,
    /// Protocol-wide statistics.
    pub stats: Arc<ServerStats>,
}

/// Everything a simulated protocol server must provide.
pub trait Protocol: Send + Sync {
    /// Stable identifier, e.g. `"modbus-tcp"`.
    fn id(&self) -> &'static str;

    /// Display name, e.g. `"Modbus TCP"`.
    fn name(&self) -> &'static str;

    /// One-line description.
    fn description(&self) -> &'static str;

    /// Start the server (synchronously returns once the process is spawned;
    /// the actual bind/accept runs in the background).
    fn start(&self, ctx: &ProtocolContext) -> anyhow::Result<()>;

    /// Stop the server and free its port.
    fn stop(&self);

    /// Current lifecycle state.
    fn state(&self) -> ServerState;

    /// True while the server process is alive.
    fn is_running(&self) -> bool {
        self.state() == ServerState::Running
    }

    /// The listen port (used by the UI to pre-fill the control card).
    fn port(&self) -> u16;

    /// Change the listen port. Only effective while stopped.
    fn set_port(&mut self, port: u16);
}

/// A registered protocol server and its UI state.
pub struct ProtocolCard {
    pub protocol: Box<dyn Protocol>,
}

impl ProtocolCard {
    pub fn new(protocol: Box<dyn Protocol>) -> Self {
        Self { protocol }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_track_peaks() {
        let s = ServerStats::default();
        s.client_connected();
        s.client_connected();
        s.client_disconnected();
        assert_eq!(s.current_clients.load(Ordering::Relaxed), 1);
        assert_eq!(s.peak_clients.load(Ordering::Relaxed), 2);
    }
}