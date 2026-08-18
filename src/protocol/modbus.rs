//! Modbus TCP simulation server.
//!
//! A from-scratch, dependency-light Modbus TCP implementation (MBAP header +
//! PDU) so the simulator stays fully under our control and easy to extend.
//!
//! Supported function codes:
//!
//! | FC  | Name                     | Area             |
//! |-----|--------------------------|------------------|
//! | 0x01| Read Coils               | Coils            |
//! | 0x02| Read Discrete Inputs     | Discrete Inputs  |
//! | 0x03| Read Holding Registers   | Holding Registers|
//! | 0x04| Read Input Registers     | Input Registers  |
//! | 0x05| Write Single Coil        | Coils            |
//! | 0x06| Write Single Register    | Holding Registers|
//! | 0x0F| Write Multiple Coils     | Coils            |
//! | 0x10| Write Multiple Registers | Holding Registers|
//!
//! Every read/write goes straight to the shared [`RegisterStore`], so changes
//! made by clients are visible in the UI in real time, and UI edits are
//! immediately served to clients.

use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use crate::model::{Area, SharedStore};

use super::{Protocol, ProtocolContext, ServerState, ServerStats};

/// Default Modbus TCP port.
pub const DEFAULT_PORT: u16 = 502;

/// Max size of a Modbus frame (MBAP 7 + one byte unit id is included in `len`).
const MAX_FRAME: usize = 260;

/// Idle timeout for a client connection before it is dropped.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// A cooperative handle keeping the tokio runtime alive for this server.
struct RuntimeHandle {
    rt: tokio::runtime::Runtime,
    _join: tokio::task::JoinHandle<()>,
}

/// Modbus TCP server implementing [`Protocol`].
pub struct ModbusTcpServer {
    port: Mutex<u16>,
    running: Arc<AtomicBool>,
    state: Arc<Mutex<ServerState>>,
    runtime: Mutex<Option<RuntimeHandle>>,
}

impl ModbusTcpServer {
    pub fn new(port: u16) -> Self {
        Self {
            port: Mutex::new(port),
            running: Arc::new(AtomicBool::new(false)),
            state: Arc::new(Mutex::new(ServerState::Stopped)),
            runtime: Mutex::new(None),
        }
    }

    fn snapshot_state(&self) -> ServerState {
        self.state.lock().unwrap().clone()
    }
}

impl Protocol for ModbusTcpServer {
    fn id(&self) -> &'static str {
        "modbus-tcp"
    }

    fn name(&self) -> &'static str {
        "Modbus TCP"
    }

    fn description(&self) -> &'static str {
        "Modbus TCP server (FC 01-06, 0F, 10)"
    }

    fn start(&self, ctx: &ProtocolContext) -> anyhow::Result<()> {
        if self.is_running() {
            return Ok(());
        }
        if self.runtime.lock().unwrap().is_some() {
            // A previous run left a handle behind (shouldn't happen).
            self.stop();
        }

        let port = *self.port.lock().unwrap();
        *self.state.lock().unwrap() = ServerState::Starting;

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("modbus-tcp")
            .enable_all()
            .build()?;

        let store = ctx.store.clone();
        let stats = ctx.stats.clone();
        let running = self.running.clone();
        let state = self.state.clone();

        running.store(true, Ordering::Relaxed);
        let handle = rt.spawn(async move {
            run_tcp_server(port, store, stats, running, state).await;
        });

        *self.runtime.lock().unwrap() = Some(RuntimeHandle { rt, _join: handle });
        log::info!("[modbus-tcp] started on port {}", port);
        Ok(())
    }

    fn stop(&self) {
        if !self.is_running() && self.runtime.lock().unwrap().is_none() {
            *self.state.lock().unwrap() = ServerState::Stopped;
            return;
        }
        self.running.store(false, Ordering::Relaxed);
        *self.state.lock().unwrap() = ServerState::Stopping;
        if let Some(handle) = self.runtime.lock().unwrap().take() {
            // Bind the fields in a block so the struct isn't partially moved:
            // `shutdown_timeout` borrows `rt`, then `handle` drops as a whole.
            let RuntimeHandle { rt, _join } = handle;
            drop(_join);
            rt.shutdown_timeout(Duration::from_millis(500));
        }
        *self.state.lock().unwrap() = ServerState::Stopped;
        log::info!("[modbus-tcp] stopped");
    }

    fn state(&self) -> ServerState {
        self.snapshot_state()
    }

    fn port(&self) -> u16 {
        *self.port.lock().unwrap()
    }

    fn set_port(&mut self, port: u16) {
        if !self.is_running() {
            *self.port.lock().unwrap() = port;
        }
    }
}

// ---------------------------------------------------------------------------
// TCP accept loop + per-connection request handling
// ---------------------------------------------------------------------------

async fn run_tcp_server(
    port: u16,
    store: SharedStore,
    stats: Arc<ServerStats>,
    running: Arc<AtomicBool>,
    state: Arc<Mutex<ServerState>>,
) {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            log::error!("[modbus-tcp] cannot bind {addr}: {e}");
            *state.lock().unwrap() = ServerState::Error(format!("绑定 {addr} 失败: {e}"));
            return;
        }
    };
    *state.lock().unwrap() = ServerState::Running;
    log::info!("[modbus-tcp] listening on {addr}");

    while running.load(Ordering::Relaxed) {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let store = store.clone();
                let stats = stats.clone();
                let running = running.clone();
                tokio::spawn(async move {
                    handle_connection(stream, peer, store, stats, running).await;
                });
            }
            Err(e) => {
                log::warn!("[modbus-tcp] accept error: {e}");
                // Transient errors (e.g. EMFILE) — retry after a short pause.
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    peer: SocketAddr,
    store: SharedStore,
    stats: Arc<ServerStats>,
    _running: Arc<AtomicBool>,
) {
    stats.client_connected();
    log::debug!("[modbus-tcp] client connected: {peer}");

    let mut header = [0u8; 7];
    let mut pdu = Vec::with_capacity(MAX_FRAME);
    loop {
        let res = tokio::time::timeout(IDLE_TIMEOUT, stream.read_exact(&mut header)).await;
        match res {
            Ok(Ok(_)) => {}
            // Timeout or IO error / EOF: drop the connection.
            _ => break,
        }

        let _tid = u16::from_be_bytes([header[0], header[1]]);
        let pid = u16::from_be_bytes([header[2], header[3]]);
        let len = u16::from_be_bytes([header[4], header[5]]);
        let uid = header[6];

        // `len` = unit id byte + PDU. Anything outside [2, 254] is malformed.
        if !(2..=254).contains(&len) {
            // Send nothing; just close the connection.
            break;
        }

        pdu.resize(len as usize - 1, 0);
        if stream.read_exact(&mut pdu).await.is_err() {
            break;
        }

        let response = if pid != 0 {
            // A Modbus TCP frame must carry protocol id 0.
            exception(0x01, 0x01) // illegal function
        } else {
            process_pdu(&pdu, &store, &stats)
        };

        stats.requests(1);

        let mut out = Vec::with_capacity(7 + response.len());
        out.extend_from_slice(&[header[0], header[1], 0x00, 0x00]);
        out.extend_from_slice(&((1 + response.len()) as u16).to_be_bytes());
        out.push(uid);
        out.extend_from_slice(&response);

        if stream.write_all(&out).await.is_err() {
            break;
        }
    }

    stats.client_disconnected();
    log::debug!("[modbus-tcp] client disconnected: {peer}");
}

// ---------------------------------------------------------------------------
// Modbus PDU processing
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // these are the standard Modbus exception names
enum ModbusError {
    IllegalFunction = 0x01,
    IllegalDataAddress = 0x02,
    IllegalDataValue = 0x03,
}

fn exception(fc: u8, code: u8) -> Vec<u8> {
    vec![0x80 | fc, code]
}

fn process_pdu(pdu: &[u8], store: &SharedStore, stats: &ServerStats) -> Vec<u8> {
    let Some(&fc) = pdu.first() else {
        return Vec::new();
    };

    let result = match fc {
        0x01 => read_bits(store, Area::Coils, &pdu[1..]),
        0x02 => read_bits(store, Area::DiscreteInputs, &pdu[1..]),
        0x03 => read_words(store, Area::HoldingRegisters, &pdu[1..]),
        0x04 => read_words(store, Area::InputRegisters, &pdu[1..]),
        0x05 => write_single_bit(store, stats, Area::Coils, &pdu[1..]),
        0x06 => write_single_word(store, stats, Area::HoldingRegisters, &pdu[1..]),
        0x0F => write_multiple_bits(store, stats, Area::Coils, &pdu[1..]),
        0x10 => write_multiple_words(store, stats, Area::HoldingRegisters, &pdu[1..]),
        _ => Err(ModbusError::IllegalFunction),
    };

    match result {
        Ok(data) => {
            let mut out = Vec::with_capacity(1 + data.len());
            out.push(fc);
            out.extend_from_slice(&data);
            out
        }
        Err(e) => {
            stats.errors(1);
            exception(fc, e as u8)
        }
    }
}

fn parse_addr_qty(data: &[u8]) -> Option<(u16, u16)> {
    if data.len() < 4 {
        return None;
    }
    let addr = u16::from_be_bytes([data[0], data[1]]);
    let qty = u16::from_be_bytes([data[2], data[3]]);
    Some((addr, qty))
}

fn check_range(store: &SharedStore, area: Area, addr: u16, qty: usize) -> Result<(), ModbusError> {
    if qty == 0 {
        return Err(ModbusError::IllegalDataValue);
    }
    let s = store.read().unwrap();
    if addr as usize + qty > s.len(area) {
        return Err(ModbusError::IllegalDataAddress);
    }
    Ok(())
}

/// FC 0x01 / 0x02 — read a bit area.
fn read_bits(store: &SharedStore, area: Area, data: &[u8]) -> Result<Vec<u8>, ModbusError> {
    let (addr, qty) = parse_addr_qty(data).ok_or(ModbusError::IllegalDataValue)?;
    if qty == 0 || qty > 2000 {
        return Err(ModbusError::IllegalDataValue);
    }
    check_range(store, area, addr, qty as usize)?;

    let values = store.read().unwrap().range(area, addr as usize, qty as usize);
    let values = values.unwrap(); // range already checked
    let byte_count = (qty as usize).div_ceil(8);
    let mut bytes = vec![0u8; byte_count];
    for (i, v) in values.iter().enumerate() {
        if *v != 0 {
            bytes[i / 8] |= 1 << (i % 8);
        }
    }
    let mut out = Vec::with_capacity(1 + bytes.len());
    out.push(byte_count as u8);
    out.extend_from_slice(&bytes);
    Ok(out)
}

/// FC 0x03 / 0x04 — read a word area.
fn read_words(store: &SharedStore, area: Area, data: &[u8]) -> Result<Vec<u8>, ModbusError> {
    let (addr, qty) = parse_addr_qty(data).ok_or(ModbusError::IllegalDataValue)?;
    if qty == 0 || qty > 125 {
        return Err(ModbusError::IllegalDataValue);
    }
    check_range(store, area, addr, qty as usize)?;

    let values = store.read().unwrap().range(area, addr as usize, qty as usize);
    let values = values.unwrap();
    let mut out = Vec::with_capacity(1 + qty as usize * 2);
    out.push((qty as usize * 2) as u8);
    for v in values {
        out.extend_from_slice(&v.to_be_bytes());
    }
    Ok(out)
}

/// FC 0x05 — write a single coil.
fn write_single_bit(
    store: &SharedStore,
    stats: &ServerStats,
    area: Area,
    data: &[u8],
) -> Result<Vec<u8>, ModbusError> {
    if data.len() < 4 {
        return Err(ModbusError::IllegalDataValue);
    }
    let addr = u16::from_be_bytes([data[0], data[1]]);
    let value = u16::from_be_bytes([data[2], data[3]]);
    if value != 0x0000 && value != 0xFF00 {
        return Err(ModbusError::IllegalDataValue);
    }
    check_range(store, area, addr, 1)?;

    let bit = if value == 0xFF00 { 1 } else { 0 };
    store.write().unwrap().set(area, addr as usize, bit, &writer(0));
    stats.cells_written(1);

    // Echo the request back.
    Ok(data[..4].to_vec())
}

/// FC 0x06 — write a single register.
fn write_single_word(
    store: &SharedStore,
    stats: &ServerStats,
    area: Area,
    data: &[u8],
) -> Result<Vec<u8>, ModbusError> {
    if data.len() < 4 {
        return Err(ModbusError::IllegalDataValue);
    }
    let addr = u16::from_be_bytes([data[0], data[1]]);
    let value = u16::from_be_bytes([data[2], data[3]]);
    check_range(store, area, addr, 1)?;

    store.write().unwrap().set(area, addr as usize, value, &writer(0));
    stats.cells_written(1);

    Ok(data[..4].to_vec())
}

/// FC 0x0F — write multiple coils.
fn write_multiple_bits(
    store: &SharedStore,
    stats: &ServerStats,
    area: Area,
    data: &[u8],
) -> Result<Vec<u8>, ModbusError> {
    let (addr, qty) = parse_addr_qty(data).ok_or(ModbusError::IllegalDataValue)?;
    if qty == 0 || qty > 1968 {
        return Err(ModbusError::IllegalDataValue);
    }
    let byte_count = (qty as usize).div_ceil(8);
    if data.len() < 5 + byte_count {
        return Err(ModbusError::IllegalDataValue);
    }
    if data[4] as usize != byte_count {
        return Err(ModbusError::IllegalDataValue);
    }
    check_range(store, area, addr, qty as usize)?;

    let mut values = vec![0u16; qty as usize];
    for (i, v) in values.iter_mut().enumerate() {
        *v = ((data[5 + i / 8] >> (i % 8)) & 1) as u16;
    }
    store.write().unwrap().set_range(area, addr as usize, &values, &writer(0));
    stats.cells_written(qty as u64);

    Ok(data[..4].to_vec())
}

/// FC 0x10 — write multiple registers.
fn write_multiple_words(
    store: &SharedStore,
    stats: &ServerStats,
    area: Area,
    data: &[u8],
) -> Result<Vec<u8>, ModbusError> {
    let (addr, qty) = parse_addr_qty(data).ok_or(ModbusError::IllegalDataValue)?;
    if qty == 0 || qty > 123 {
        return Err(ModbusError::IllegalDataValue);
    }
    let byte_count = qty as usize * 2;
    if data.len() < 5 + byte_count {
        return Err(ModbusError::IllegalDataValue);
    }
    if data[4] as usize != byte_count {
        return Err(ModbusError::IllegalDataValue);
    }
    check_range(store, area, addr, qty as usize)?;

    let mut values = Vec::with_capacity(qty as usize);
    for i in 0..qty as usize {
        let off = 5 + i * 2;
        values.push(u16::from_be_bytes([data[off], data[off + 1]]));
    }
    store.write().unwrap().set_range(area, addr as usize, &values, &writer(0));
    stats.cells_written(qty as u64);

    Ok(data[..4].to_vec())
}

/// Writer label stamped into the store. The unit id would also be useful;
/// keep it static for a compact UI column.
fn writer(_uid: u8) -> String {
    "Modbus TCP".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> SharedStore {
        Arc::new(std::sync::RwLock::new(crate::model::RegisterStore::new(256)))
    }

    #[test]
    fn read_holding_registers_roundtrip() {
        let store = test_store();
        store.write().unwrap().set(Area::HoldingRegisters, 3, 0x1234, "test");
        store.write().unwrap().set(Area::HoldingRegisters, 4, 0xABCD, "test");

        let pdu = [0x03, 0x00, 0x03, 0x00, 0x02];
        let resp = process_pdu(&pdu, &store, &ServerStats::default());
        assert_eq!(resp, vec![0x03, 0x04, 0x12, 0x34, 0xAB, 0xCD]);
    }

    #[test]
    fn write_multiple_registers_then_read() {
        let store = test_store();
        let pdu = [
            0x10, 0x00, 0x00, 0x00, 0x02, 0x04, 0x00, 0x0A, 0x00, 0x0B,
        ];
        let resp = process_pdu(&pdu, &store, &ServerStats::default());
        assert_eq!(resp, vec![0x10, 0x00, 0x00, 0x00, 0x02]);

        let pdu = [0x03, 0x00, 0x00, 0x00, 0x02];
        let resp = process_pdu(&pdu, &store, &ServerStats::default());
        assert_eq!(resp, vec![0x03, 0x04, 0x00, 0x0A, 0x00, 0x0B]);
    }

    #[test]
    fn read_coils_bit_packing() {
        let store = test_store();
        store.write().unwrap().set(Area::Coils, 0, 1, "test");
        store.write().unwrap().set(Area::Coils, 7, 1, "test");
        store.write().unwrap().set(Area::Coils, 8, 1, "test");

        let pdu = [0x01, 0x00, 0x00, 0x00, 0x09];
        let resp = process_pdu(&pdu, &store, &ServerStats::default());
        assert_eq!(resp, vec![0x01, 0x02, 0x81, 0x01]);
    }

    #[test]
    fn out_of_bounds_read_returns_exception() {
        let store = test_store();
        let pdu = [0x03, 0x01, 0x00, 0x00, 0x10]; // addr 256 + qty 16 > 256
        let resp = process_pdu(&pdu, &store, &ServerStats::default());
        assert_eq!(resp, vec![0x83, 0x02]); // illegal data address
    }

    #[test]
    fn bad_quantity_returns_exception() {
        let store = test_store();
        let pdu = [0x03, 0x00, 0x00, 0x00, 0x00]; // qty 0
        let resp = process_pdu(&pdu, &store, &ServerStats::default());
        assert_eq!(resp, vec![0x83, 0x03]); // illegal data value
    }

    #[test]
    fn write_single_coil_echo() {
        let store = test_store();
        let pdu = [0x05, 0x00, 0x0A, 0xFF, 0x00];
        let resp = process_pdu(&pdu, &store, &ServerStats::default());
        assert_eq!(resp, vec![0x05, 0x00, 0x0A, 0xFF, 0x00]);
        assert_eq!(store.read().unwrap().get(Area::Coils, 10), Some(1));
    }
}