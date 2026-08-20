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

use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc, RwLock,
};
use std::time::{SystemTime, UNIX_EPOCH};

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
            match self.peak_clients.compare_exchange_weak(
                peak,
                cur,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
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

/// Direction of a logged protocol message, as seen by the simulator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MsgDirection {
    /// A message our server sent back to a client.
    Sent,
    /// A message our server received from a client.
    Received,
}

impl MsgDirection {
    /// Short UI label.
    pub fn label(self) -> &'static str {
        match self {
            MsgDirection::Sent => "发送",
            MsgDirection::Received => "接收",
        }
    }
}

/// One logged protocol frame (request/response) shown in the UI log panel.
#[derive(Clone, Debug)]
pub struct MessageLogEntry {
    pub direction: MsgDirection,
    /// Function code (0 if not applicable).
    pub function_code: u8,
    /// Total frame length in bytes.
    pub bytes: usize,
    /// Number of 16-bit register words in the payload (0 when not applicable).
    pub registers: usize,
    /// Full message rendered as big-endian hex.
    pub hex: String,
    /// `HH:MM:SS` wall-clock time.
    pub time: String,
    /// Milliseconds since the UNIX epoch. Kept alongside [`Self::time`] so the
    /// log table can sort entries reliably by generation / receive time.
    pub ts_ms: u64,
}

/// Max entries kept in the shared message log (ring buffer).
pub const MAX_LOG_ENTRIES: usize = 200;

/// A shared, capped, thread-safe ring buffer of logged protocol frames.
pub type SharedMessageLog = Arc<RwLock<VecDeque<MessageLogEntry>>>;

/// Create an empty shared message log.
pub fn shared_message_log() -> SharedMessageLog {
    Arc::new(RwLock::new(VecDeque::with_capacity(MAX_LOG_ENTRIES)))
}

/// Push a frame into the log, evicting the oldest entry once full.
pub fn log_message(logs: &SharedMessageLog, entry: MessageLogEntry) {
    if let Ok(mut q) = logs.write() {
        if q.len() >= MAX_LOG_ENTRIES {
            q.pop_front();
        }
        q.push_back(entry);
    }
}

/// Current `HH:MM:SS` timestamp.
pub fn timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

/// Milliseconds since the UNIX epoch (monotonic-enough for log ordering when
/// combined with the wall-clock [`timestamp`] label).
pub fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
    /// Shared message log (received/sent frames) for the UI log panel.
    pub logs: SharedMessageLog,
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

    #[test]
    fn message_log_ring_buffer_caps_and_orders() {
        let logs = shared_message_log();
        for i in 0..(MAX_LOG_ENTRIES + 20) {
            log_message(
                &logs,
                MessageLogEntry {
                    direction: if i % 2 == 0 {
                        MsgDirection::Sent
                    } else {
                        MsgDirection::Received
                    },
                    function_code: (i % 0x10) as u8,
                    bytes: i + 7,
                    registers: i,
                    hex: format!("{i:02X}"),
                    time: timestamp(),
                    ts_ms: timestamp_ms(),
                },
            );
        }
        let q = logs.read().unwrap();
        assert_eq!(q.len(), MAX_LOG_ENTRIES);
        // The (N+20)th entry has function_code = (MAX_LOG_ENTRIES+19) % 0x10.
        assert_eq!(
            q.back().unwrap().function_code,
            ((MAX_LOG_ENTRIES + 19) % 0x10) as u8
        );
        assert_eq!(q.front().unwrap().registers, 20);
    }
}
