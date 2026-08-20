//! End-to-end test: run the real Modbus TCP server on an ephemeral port and
//! exercise it with a raw TCP client (MBAP + PDU), verifying that writes are
//! visible to the shared store and reads return correct values.

use std::sync::{atomic::Ordering, Arc};

use iot_mock::model::{shared_store, Area};
use iot_mock::protocol::{modbus::ModbusTcpServer, Protocol, ProtocolContext, ServerStats};

fn free_port() -> u16 {
    // Bind to port 0 to get an OS-assigned ephemeral port, then drop it.
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// One Modbus TCP request round trip: send MBAP+PDU, read MBAP+PDU response.
fn modbus_request(stream: &mut std::net::TcpStream, tid: u16, unit: u8, pdu: &[u8]) -> Vec<u8> {
    use std::io::{Read, Write};
    let mut tx = Vec::with_capacity(7 + pdu.len());
    tx.extend_from_slice(&tid.to_be_bytes());
    tx.extend_from_slice(&[0x00, 0x00]);
    tx.extend_from_slice(&((1 + pdu.len()) as u16).to_be_bytes());
    tx.push(unit);
    tx.extend_from_slice(pdu);
    stream.write_all(&tx).unwrap();

    let mut header = [0u8; 7];
    stream.read_exact(&mut header).unwrap();
    let len = u16::from_be_bytes([header[4], header[5]]) as usize;
    assert!(len >= 1);
    let mut body = vec![0u8; len - 1];
    stream.read_exact(&mut body).unwrap();
    body
}

#[test]
fn server_end_to_end_reads_and_writes() {
    let store = shared_store(256);
    let stats = Arc::new(ServerStats::default());
    let port = free_port();
    let server = ModbusTcpServer::new(port);

    let started = server.start(&ProtocolContext {
        store: store.clone(),
        stats: stats.clone(),
        logs: iot_mock::protocol::shared_message_log(),
    });
    assert!(started.is_ok(), "start failed: {started:?}");

    // Wait for the accept loop to bind.
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        server.is_running() || matches!(server.state(), iot_mock::protocol::ServerState::Error(_)),
        "server not running: {:?}",
        server.state()
    );

    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    stream
        .set_write_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();

    // Write multiple registers (FC 0x10) at addr 0: 0x1111, 0x2222, 0x3333.
    let write_pdu = [
        0x10, 0x00, 0x00, 0x00, 0x03, 0x06, 0x11, 0x11, 0x22, 0x22, 0x33, 0x33,
    ];
    let resp = modbus_request(&mut stream, 0x0001, 0x01, &write_pdu);
    assert_eq!(resp, vec![0x10, 0x00, 0x00, 0x00, 0x03]);

    // Read them back (FC 0x03).
    let read_pdu = [0x03, 0x00, 0x00, 0x00, 0x03];
    let resp = modbus_request(&mut stream, 0x0002, 0x01, &read_pdu);
    assert_eq!(resp, vec![0x03, 0x06, 0x11, 0x11, 0x22, 0x22, 0x33, 0x33]);

    // The shared store must reflect the client's writes.
    {
        let s = store.read().unwrap();
        assert_eq!(s.get(Area::HoldingRegisters, 0), Some(0x1111));
        assert_eq!(s.get(Area::HoldingRegisters, 2), Some(0x3333));
        assert_eq!(s.cells[2][0].writer, "Modbus TCP");
    }

    // Write a single coil (FC 0x05) and single register (FC 0x06).
    let coil_pdu = [0x05, 0x00, 0x0A, 0xFF, 0x00];
    let resp = modbus_request(&mut stream, 0x0003, 0x01, &coil_pdu);
    assert_eq!(resp, vec![0x05, 0x00, 0x0A, 0xFF, 0x00]);
    let reg_pdu = [0x06, 0x00, 0x14, 0xAB, 0xCD];
    let resp = modbus_request(&mut stream, 0x0004, 0x01, &reg_pdu);
    assert_eq!(resp, vec![0x06, 0x00, 0x14, 0xAB, 0xCD]);
    {
        let s = store.read().unwrap();
        assert_eq!(s.get(Area::Coils, 10), Some(1));
        assert_eq!(s.get(Area::HoldingRegisters, 20), Some(0xABCD));
    }

    // Unit id is echoed and preserved across the frame.
    let resp = modbus_request(&mut stream, 0x0005, 0x42, &read_pdu);
    assert_eq!(resp.len(), 8);

    // Out-of-bounds read yields an exception (FC 0x83, code 0x02).
    let bad_pdu = [0x03, 0x01, 0x00, 0x00, 0x04]; // addr 256, qty 4 > 256 cells
    let resp = modbus_request(&mut stream, 0x0006, 0x01, &bad_pdu);
    assert_eq!(resp, vec![0x83, 0x02]);

    // Stats should have recorded the requests and errors.
    assert!(stats.total_requests.load(Ordering::Relaxed) >= 6);
    assert!(stats.error_responses.load(Ordering::Relaxed) >= 1);
    assert!(stats.cells_written.load(Ordering::Relaxed) >= 5);

    drop(stream);
    server.stop();
    assert!(!server.is_running());
    assert_eq!(server.state(), iot_mock::protocol::ServerState::Stopped);
}
