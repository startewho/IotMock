//! IoT protocol simulator library.
//!
//! Provides the shared data model and protocol abstraction so protocol servers
//! (Modbus TCP today, S7 / OPC-UA tomorrow) can be tested and used headlessly,
//! independent of the GPUI desktop shell.

pub mod model;
pub mod protocol;
