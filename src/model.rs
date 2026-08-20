//! Shared data model for the IoT simulator.
//!
//! The [`RegisterStore`] is the single source of truth for all register data.
//! It is shared (via `Arc<RwLock<...>>`) between:
//! - protocol servers (run on background tokio runtimes)
//! - the GPUI application (run on the UI thread)
//!
//! This makes every protocol view of the data consistent with what the UI
//! displays, and any write (from a Modbus client or from the UI) is visible
//! everywhere in real time.

use std::sync::{Arc, RwLock};

/// Default number of cells per data area.
pub const DEFAULT_AREA_SIZE: usize = 1024;

/// Data width of a register value: 16-bit (one register) or 32-bit (two
/// consecutive registers). Register data has no intrinsic type, so the user
/// decides on a per-edit basis.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DataWidth {
    /// 16 bits = 1 register (1 × 16-bit word).
    #[default]
    Bits16,
    /// 32 bits = 2 consecutive registers.
    Bits32,
}

impl DataWidth {
    /// Number of registers occupied.
    pub fn registers(self) -> usize {
        match self {
            DataWidth::Bits16 => 1,
            DataWidth::Bits32 => 2,
        }
    }

    /// Number of bits.
    pub fn bits(self) -> usize {
        match self {
            DataWidth::Bits16 => 16,
            DataWidth::Bits32 => 32,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DataWidth::Bits16 => "16 位 (单寄存器)",
            DataWidth::Bits32 => "32 位 (双寄存器)",
        }
    }
}

/// The four standard Modbus / S7 byte orders for a 32-bit value that spans two
/// 16-bit registers.
///
/// Let a 32-bit value `v` have bytes (MSB→LSB) `B3 B2 B1 B0`, and let `W0` be
/// the high 16-bit word (bytes `B3 B2`) and `W1` the low word (bytes `B1 B0`).
/// The supported layouts, per [Modbus Application Protocol](https://modbus.org/docs/Modbus_Application_Protocol_V1_1b3.pdf)
/// and S7/SIMATIC conventions, are:
///
/// | Order | 寄存器0 (首地址) | 寄存器1 | 说明 |
/// |-------|------------------|---------|------|
/// | ABCD  | `W0` (B3 B2)     | `W1` (B1 B0) | 大端 Big-Endian |
/// | CDAB  | `W1` (B1 B0)     | `W0` (B3 B2) | 小端 Little-Endian (字交换) |
/// | BADC  | `B2 B3`          | `B0 B1`     | 大端字交换 (字节逆序/字内交换) |
/// | DCBA  | `B0 B1`          | `B2 B3`     | 小端字节+字交换 |
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ByteOrder {
    /// ABCD — Big-Endian. Register0 = high word.
    #[default]
    Abcd,
    /// CDAB — Little-Endian. Register0 = low word.
    Cdab,
    /// BADC — Big-Endian with word-swap.
    Badc,
    /// DCBA — Little-Endian with byte and word swap.
    Dcba,
}

impl ByteOrder {
    /// All supported orders, for the select dropdown.
    pub const ALL: [ByteOrder; 4] = [
        ByteOrder::Abcd,
        ByteOrder::Cdab,
        ByteOrder::Badc,
        ByteOrder::Dcba,
    ];

    /// Display label.
    pub fn name(self) -> &'static str {
        match self {
            ByteOrder::Abcd => "ABCD (大端 Big-Endian)",
            ByteOrder::Cdab => "CDAB (小端 Little-Endian)",
            ByteOrder::Badc => "BADC (大端字交换 Word-Swap)",
            ByteOrder::Dcba => "DCBA (小端字节+字交换)",
        }
    }

    /// Short token.
    pub fn code(self) -> &'static str {
        match self {
            ByteOrder::Abcd => "ABCD",
            ByteOrder::Cdab => "CDAB",
            ByteOrder::Badc => "BADC",
            ByteOrder::Dcba => "DCBA",
        }
    }

    /// Encode a 32-bit value into the two 16-bit register words placed at
    /// `addr` and `addr+1` using this byte order.
    pub fn encode_u32(self, value: u32) -> [u16; 2] {
        let b0 = (value & 0xFF) as u8;
        let b1 = ((value >> 8) & 0xFF) as u8;
        let b2 = ((value >> 16) & 0xFF) as u8;
        let b3 = ((value >> 24) & 0xFF) as u8;
        match self {
            ByteOrder::Abcd => [u16::from_be_bytes([b3, b2]), u16::from_be_bytes([b1, b0])],
            ByteOrder::Cdab => [u16::from_be_bytes([b1, b0]), u16::from_be_bytes([b3, b2])],
            ByteOrder::Badc => [u16::from_be_bytes([b2, b3]), u16::from_be_bytes([b0, b1])],
            ByteOrder::Dcba => [u16::from_be_bytes([b0, b1]), u16::from_be_bytes([b2, b3])],
        }
    }

    /// Decode two 16-bit register words read from `addr` and `addr+1` back into
    /// a 32-bit value using this byte order.
    pub fn decode_u32(self, words: [u16; 2]) -> u32 {
        let (r0, r1) = (words[0], words[1]);
        match self {
            ByteOrder::Abcd => {
                let b3 = (r0 >> 8) as u8;
                let b2 = (r0 & 0xFF) as u8;
                let b1 = (r1 >> 8) as u8;
                let b0 = (r1 & 0xFF) as u8;
                u32::from_be_bytes([b3, b2, b1, b0])
            }
            ByteOrder::Cdab => {
                let b3 = (r1 >> 8) as u8;
                let b2 = (r1 & 0xFF) as u8;
                let b1 = (r0 >> 8) as u8;
                let b0 = (r0 & 0xFF) as u8;
                u32::from_be_bytes([b3, b2, b1, b0])
            }
            ByteOrder::Badc => {
                let b3 = (r0 & 0xFF) as u8;
                let b2 = (r0 >> 8) as u8;
                let b1 = (r1 & 0xFF) as u8;
                let b0 = (r1 >> 8) as u8;
                u32::from_be_bytes([b3, b2, b1, b0])
            }
            ByteOrder::Dcba => {
                let b3 = (r1 & 0xFF) as u8;
                let b2 = (r1 >> 8) as u8;
                let b1 = (r0 & 0xFF) as u8;
                let b0 = (r0 >> 8) as u8;
                u32::from_be_bytes([b3, b2, b1, b0])
            }
        }
    }

    /// Encode a 64-bit value into the four 16-bit register words using this byte
    /// order. The 32-bit order is applied independently to the high and low
    /// 32-bit halves (each occupying two consecutive registers).
    pub fn encode_u64(self, value: u64) -> [u16; 4] {
        let hi = value >> 32;
        let lo = value & 0xFFFF_FFFF;
        let [a, b] = self.encode_u32(hi as u32);
        let [c, d] = self.encode_u32(lo as u32);
        [a, b, c, d]
    }

    /// Decode four 16-bit register words back into a 64-bit value using this
    /// byte order (inverse of [`ByteOrder::encode_u64`]).
    pub fn decode_u64(self, words: [u16; 4]) -> u64 {
        let hi = self.decode_u32([words[0], words[1]]) as u64;
        let lo = self.decode_u32([words[2], words[3]]) as u64;
        (hi << 32) | lo
    }
}

/// A typed interpretation of one or more registers, used by the edit dialog to
/// "auto-fill" register values from a human-friendly value (number, float or
/// UTF-8 string).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ValueType {
    /// 16-bit unsigned (1 register).
    #[default]
    Uint16,
    /// 16-bit signed (1 register).
    Int16,
    /// 32-bit unsigned (2 registers).
    Uint32,
    /// 32-bit signed (2 registers).
    Int32,
    /// 32-bit IEEE-754 float (2 registers).
    Float32,
    /// 64-bit unsigned (4 registers).
    Uint64,
    /// 64-bit signed (4 registers).
    Int64,
    /// 64-bit IEEE-754 double (4 registers).
    Double,
    /// UTF-8 string packed 2 bytes per register (variable register count).
    String,
}

impl ValueType {
    pub const ALL: [ValueType; 9] = [
        ValueType::Uint16,
        ValueType::Int16,
        ValueType::Uint32,
        ValueType::Int32,
        ValueType::Float32,
        ValueType::Uint64,
        ValueType::Int64,
        ValueType::Double,
        ValueType::String,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ValueType::Uint16 => "UInt16",
            ValueType::Int16 => "Int16",
            ValueType::Uint32 => "UInt32",
            ValueType::Int32 => "Int32",
            ValueType::Float32 => "Float32",
            ValueType::Uint64 => "UInt64",
            ValueType::Int64 => "Int64",
            ValueType::Double => "Double",
            ValueType::String => "String",
        }
    }

    pub fn label_zh(self) -> &'static str {
        match self {
            ValueType::Uint16 => "无符号16位整数",
            ValueType::Int16 => "有符号16位整数",
            ValueType::Uint32 => "无符号32位整数",
            ValueType::Int32 => "有符号32位整数",
            ValueType::Float32 => "32位浮点数",
            ValueType::Uint64 => "无符号64位整数",
            ValueType::Int64 => "有符号64位整数",
            ValueType::Double => "64位双精度浮点数",
            ValueType::String => "字符串 / 字符",
        }
    }

    /// Register count of a fixed-width type; `None` for variable-length
    /// (string) types.
    pub fn fixed_registers(self) -> Option<usize> {
        match self {
            ValueType::Uint16 | ValueType::Int16 => Some(1),
            ValueType::Uint32 | ValueType::Int32 | ValueType::Float32 => Some(2),
            ValueType::Uint64 | ValueType::Int64 | ValueType::Double => Some(4),
            ValueType::String => None,
        }
    }

    /// Number of bits in a fixed-width type.
    pub fn bits(self) -> Option<usize> {
        self.fixed_registers().map(|n| n * 16)
    }

    /// Parse `text` for this type and produce the register words to write at
    /// `addr` (which advance for string types), applying `byte_order` for 32-bit
    /// types. `addr` must already be in bounds; string types stay within
    /// `max_regs` total registers.
    pub fn encode_text(
        self,
        text: &str,
        byte_order: ByteOrder,
        max_regs: usize,
    ) -> Result<Vec<u16>, String> {
        match self {
            ValueType::Uint16 => {
                let v: u16 = text
                    .trim()
                    .parse()
                    .map_err(|_| "无效的 UInt16 数值".to_string())?;
                Ok(vec![v])
            }
            ValueType::Int16 => {
                let v: i16 = text
                    .trim()
                    .parse()
                    .map_err(|_| "无效的 Int16 数值".to_string())?;
                Ok(vec![v as u16])
            }
            ValueType::Uint32 => {
                let v: u32 = text
                    .trim()
                    .parse()
                    .map_err(|_| "无效的 UInt32 数值".to_string())?;
                Ok(byte_order.encode_u32(v).to_vec())
            }
            ValueType::Int32 => {
                let v: i32 = text
                    .trim()
                    .parse()
                    .map_err(|_| "无效的 Int32 数值".to_string())?;
                Ok(byte_order.encode_u32(v as u32).to_vec())
            }
            ValueType::Float32 => {
                let v: f32 = text
                    .trim()
                    .parse()
                    .map_err(|_| "无效的 Float32 数值".to_string())?;
                if !v.is_finite() {
                    return Err("浮点数必须为有限值".to_string());
                }
                Ok(byte_order.encode_u32(v.to_bits()).to_vec())
            }
            ValueType::Uint64 => {
                let v: u64 = text
                    .trim()
                    .parse()
                    .map_err(|_| "无效的 UInt64 数值".to_string())?;
                Ok(byte_order.encode_u64(v).to_vec())
            }
            ValueType::Int64 => {
                let v: i64 = text
                    .trim()
                    .parse()
                    .map_err(|_| "无效的 Int64 数值".to_string())?;
                Ok(byte_order.encode_u64(v as u64).to_vec())
            }
            ValueType::Double => {
                let v: f64 = text
                    .trim()
                    .parse()
                    .map_err(|_| "无效的 Double 数值".to_string())?;
                if !v.is_finite() {
                    return Err("浮点数必须为有限值".to_string());
                }
                Ok(byte_order.encode_u64(v.to_bits()).to_vec())
            }
            ValueType::String => store_string(text, byte_order, max_regs),
        }
    }

    /// Decode `words` (starting at register 0 of the value) back to a display
    /// string for this type.
    pub fn decode_words(self, words: &[u16], byte_order: ByteOrder) -> String {
        match self {
            ValueType::Uint16 => words.first().copied().unwrap_or(0).to_string(),
            ValueType::Int16 => (words.first().copied().unwrap_or(0) as i16).to_string(),
            ValueType::Uint32 => byte_order.decode_u32([words[0], words[1]]).to_string(),
            ValueType::Int32 => (byte_order.decode_u32([words[0], words[1]]) as i32).to_string(),
            ValueType::Float32 => {
                let bits = byte_order.decode_u32([words[0], words[1]]);
                f32::from_bits(bits).to_string()
            }
            ValueType::Uint64 => byte_order
                .decode_u64([words[0], words[1], words[2], words[3]])
                .to_string(),
            ValueType::Int64 => {
                (byte_order.decode_u64([words[0], words[1], words[2], words[3]]) as i64).to_string()
            }
            ValueType::Double => {
                let bits = byte_order.decode_u64([words[0], words[1], words[2], words[3]]);
                f64::from_bits(bits).to_string()
            }
            ValueType::String => load_string(words, byte_order),
        }
    }
}

/// Number of 16-bit registers needed to hold `bytes` bytes (2 bytes per word).
pub fn bytes_to_regs(bytes: usize) -> usize {
    (bytes + 1) / 2
}

/// Encode a UTF-8 string into registers (2 bytes per register, big-endian per
/// word), advancing over `max_regs` registers. Returns an error if the string
/// needs more registers than allowed.
fn store_string(s: &str, byte_order: ByteOrder, max_regs: usize) -> Result<Vec<u16>, String> {
    let mut bytes = s.as_bytes().to_vec();
    // Pad with a NUL terminator to an even length.
    if !bytes.len().is_multiple_of(2) {
        bytes.push(0);
    }
    let n = bytes.len() / 2;
    if n > max_regs {
        return Err(format!("字符串需占用 {n} 个寄存器，超出上限 {max_regs} 个"));
    }
    Ok((0..n)
        .map(|i| match byte_order {
            ByteOrder::Abcd | ByteOrder::Badc => {
                u16::from_be_bytes([bytes[2 * i], bytes[2 * i + 1]])
            }
            ByteOrder::Cdab | ByteOrder::Dcba => {
                u16::from_be_bytes([bytes[2 * i + 1], bytes[2 * i]])
            }
        })
        .collect())
}

/// Encode a UTF-8 string into a fixed-width buffer of `max_bytes` bytes,
/// honouring that capacity. This backs `String(N)` values: the value occupies
/// `bytes_to_regs(N)` registers (a short string is NUL-padded to the full
/// width). Errors when the string is longer than the buffer or the registers
/// would run past `max_regs`.
pub fn encode_string_fixed(
    s: &str,
    byte_order: ByteOrder,
    max_regs: usize,
    max_bytes: usize,
) -> Result<Vec<u16>, String> {
    let n = s.as_bytes().len();
    if n > max_bytes {
        return Err(format!(
            "字符串长度超出上限：{n} 字节 > 上限 {max_bytes} 字节"
        ));
    }
    let width = bytes_to_regs(max_bytes);
    if width > max_regs {
        return Err(format!(
            "字符串需占用 {width} 个寄存器，超出上限 {max_regs} 个"
        ));
    }
    let mut buf = s.as_bytes().to_vec();
    while buf.len() < max_bytes {
        buf.push(0);
    }
    if !buf.len().is_multiple_of(2) {
        buf.push(0);
    }
    let words = buf.len() / 2;
    Ok((0..words)
        .map(|i| match byte_order {
            ByteOrder::Abcd | ByteOrder::Badc => u16::from_be_bytes([buf[2 * i], buf[2 * i + 1]]),
            ByteOrder::Cdab | ByteOrder::Dcba => u16::from_be_bytes([buf[2 * i + 1], buf[2 * i]]),
        })
        .collect())
}

/// Decode registers holding 2 UTF-8 bytes each back into a string, trimming a
/// trailing NUL.
fn load_string(words: &[u16], byte_order: ByteOrder) -> String {
    let mut bytes = Vec::with_capacity(words.len() * 2);
    for &w in words {
        let (hi, lo) = match byte_order {
            ByteOrder::Abcd | ByteOrder::Badc => ((w >> 8) as u8, (w & 0xFF) as u8),
            ByteOrder::Cdab | ByteOrder::Dcba => ((w & 0xFF) as u8, (w >> 8) as u8),
        };
        bytes.push(hi);
        bytes.push(lo);
    }
    // Trim single trailing NUL pad.
    let end = bytes.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    String::from_utf8_lossy(&bytes[..end]).to_string()
}

///
/// The four classic Modbus data areas.
///
/// A future S7 protocol keeps its own memory model but can *map* onto these
/// same areas (e.g. DB1 ↔ Holding Registers), so the UI stays reusable.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Area {
    /// 0x - Coils, bit, read-write   (FC 0x01 / 0x05 / 0x0F)
    Coils = 0,
    /// 1x - Discrete inputs, bit, read-only (FC 0x02)
    DiscreteInputs = 1,
    /// 3x - Holding registers, 16-bit, read-write (FC 0x03 / 0x06 / 0x10)
    HoldingRegisters = 2,
    /// 4x - Input registers, 16-bit, read-only (FC 0x04)
    InputRegisters = 3,
}

pub const ALL_AREAS: [Area; 4] = [
    Area::Coils,
    Area::DiscreteInputs,
    Area::HoldingRegisters,
    Area::InputRegisters,
];

impl Area {
    pub fn index(self) -> usize {
        self as usize
    }

    /// True for the two bit areas (coils / discrete inputs).
    pub fn is_bit(self) -> bool {
        matches!(self, Area::Coils | Area::DiscreteInputs)
    }

    /// True if the area can be written (by clients and by the UI).
    pub fn writable(self) -> bool {
        matches!(self, Area::Coils | Area::HoldingRegisters)
    }

    /// Short display name.
    #[allow(dead_code)]
    pub fn name(self) -> &'static str {
        match self {
            Area::Coils => "Coils",
            Area::DiscreteInputs => "Discrete Inputs",
            Area::HoldingRegisters => "Holding Registers",
            Area::InputRegisters => "Input Registers",
        }
    }

    /// Chinese display name.
    pub fn name_zh(self) -> &'static str {
        match self {
            Area::Coils => "线圈",
            Area::DiscreteInputs => "离散输入",
            Area::HoldingRegisters => "保持寄存器",
            Area::InputRegisters => "输入寄存器",
        }
    }

    /// Default cell name prefix.
    pub fn prefix(self) -> &'static str {
        match self {
            Area::Coils => "COIL",
            Area::DiscreteInputs => "DI",
            Area::HoldingRegisters => "HR",
            Area::InputRegisters => "IR",
        }
    }
}

/// A single addressable cell: one coil bit or one 16-bit register word.
#[derive(Clone)]
pub struct Cell {
    /// Current value. For bit areas only `0` and `1` are meaningful.
    pub value: u16,
    /// Who wrote this value last (`"Modbus TCP"`, `"UI"`, `"Simulator"`, ...).
    pub writer: String,
    /// Human friendly name, e.g. `HR_0123`.
    pub name: String,
}

/// The complete register bank.
#[derive(Clone)]
pub struct RegisterStore {
    /// One vector per [`Area`], indexable with `Area::index()`.
    pub cells: [Vec<Cell>; 4],
    /// Monotonically increasing revision, bumped on every mutation.
    /// Used by the UI to detect changes cheaply.
    pub revision: u64,
}

impl RegisterStore {
    /// Create a store with `size` cells per area, all zeroed.
    pub fn new(size: usize) -> Self {
        let size = size.max(1);
        let build = |prefix: &str| {
            (0..size)
                .map(|i| Cell {
                    value: 0,
                    writer: "—".to_string(),
                    name: format!("{prefix}_{i:04}"),
                })
                .collect::<Vec<_>>()
        };
        Self {
            cells: [
                build(Area::Coils.prefix()),
                build(Area::DiscreteInputs.prefix()),
                build(Area::HoldingRegisters.prefix()),
                build(Area::InputRegisters.prefix()),
            ],
            revision: 0,
        }
    }

    /// Number of cells in an area.
    pub fn len(&self, area: Area) -> usize {
        self.cells[area.index()].len()
    }

    /// Read a single cell.
    pub fn get(&self, area: Area, addr: usize) -> Option<u16> {
        Some(self.cells[area.index()].get(addr)?.value)
    }

    /// Read a contiguous range of cells (`addr .. addr+qty`).
    /// Returns `None` when the range is out of bounds.
    pub fn range(&self, area: Area, addr: usize, qty: usize) -> Option<Vec<u16>> {
        if qty == 0 || addr.checked_add(qty)? > self.len(area) {
            return None;
        }
        let cells = &self.cells[area.index()];
        Some(cells[addr..addr + qty].iter().map(|c| c.value).collect())
    }

    /// Write a single cell. Returns `false` when out of bounds.
    pub fn set(&mut self, area: Area, addr: usize, value: u16, writer: &str) -> bool {
        let area_cells = self.cells[area.index()].get_mut(addr);
        let Some(cell) = area_cells else {
            return false;
        };
        let value = if area.is_bit() { value & 0x1 } else { value };
        if cell.value != value || cell.writer != writer {
            cell.value = value;
            cell.writer = writer.to_string();
            self.revision = self.revision.wrapping_add(1);
        }
        true
    }

    /// Write a range of cells. Returns `false` when out of bounds (no partial write).
    pub fn set_range(&mut self, area: Area, addr: usize, values: &[u16], writer: &str) -> bool {
        if values.is_empty()
            || addr
                .checked_add(values.len())
                .is_none_or(|end| end > self.len(area))
        {
            return false;
        }
        let cells = &mut self.cells[area.index()];
        for (i, v) in values.iter().enumerate() {
            let cell = &mut cells[addr + i];
            let v = if area.is_bit() { v & 0x1 } else { *v };
            if cell.value != v || cell.writer != writer {
                cell.value = v;
                cell.writer = writer.to_string();
            }
        }
        self.revision = self.revision.wrapping_add(1);
        true
    }

    /// Fill an area with a repeating pattern (for testing / demo).
    #[allow(dead_code)]
    pub fn fill(&mut self, area: Area, values: &[u16], writer: &str) {
        if values.is_empty() {
            return;
        }
        let cells = &mut self.cells[area.index()];
        for (i, cell) in cells.iter_mut().enumerate() {
            let v = if area.is_bit() {
                values[i % values.len()] & 0x1
            } else {
                values[i % values.len()]
            };
            if cell.value != v || cell.writer != writer {
                cell.value = v;
                cell.writer = writer.to_string();
            }
        }
        self.revision = self.revision.wrapping_add(1);
    }

    /// Reset one area to all-zero.
    pub fn reset_area(&mut self, area: Area) {
        let cells = &mut self.cells[area.index()];
        let mut changed = false;
        for cell in cells.iter_mut() {
            if cell.value != 0 {
                cell.value = 0;
                changed = true;
            }
            if cell.writer != "UI" {
                cell.writer = "UI".to_string();
                changed = true;
            }
        }
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Demo mode: deterministically "wiggle" some holding-register cells so the
    /// real-time UI visibly updates even without any TCP client connected.
    /// `seed` is the tick counter; the pseudo-random sequence is reproducible.
    pub fn simulate_tick(&mut self, seed: u64) {
        let mut x = seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(0x1234_5678_9ABC_DEF0);
        let n = self.len(Area::HoldingRegisters);
        if n == 0 {
            return;
        }
        let mut writes = 0usize;
        for _ in 0..16 {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let addr = (x >> 33) as usize % n;
            let v = ((x >> 17) ^ (x >> 1)) as u16;
            if self.cells[Area::HoldingRegisters.index()][addr].value != v {
                self.cells[Area::HoldingRegisters.index()][addr].value = v;
                self.cells[Area::HoldingRegisters.index()][addr].writer = "Simulator".into();
                writes += 1;
            }
        }
        // Also wiggle a couple of coils so the bit area is alive in demo mode.
        let nc = self.len(Area::Coils);
        if nc > 0 {
            for k in 0..4 {
                let addr = (x.wrapping_add(k as u64 * 7919)) as usize % nc;
                let v = ((x >> (3 + k * 7)) & 1) as u16;
                if self.cells[Area::Coils.index()][addr].value != v {
                    self.cells[Area::Coils.index()][addr].value = v;
                    self.cells[Area::Coils.index()][addr].writer = "Simulator".into();
                    writes += 1;
                }
            }
        }
        if writes > 0 {
            self.revision = self.revision.wrapping_add(1);
        }
    }
}

/// A point-in-time row for the UI table.
#[derive(Clone, Debug)]
pub struct Row {
    /// 0-based address of the cell.
    pub addr: usize,
    pub name: String,
    pub value: u16,
    /// Who wrote the value last.
    pub writer: String,
    /// True when the value changed since the previous snapshot (for highlight).
    pub changed: bool,
}

/// A cheap point-in-time snapshot of one area, produced on the UI thread.
#[derive(Clone, Debug)]
pub struct AreaSnapshot {
    pub area: Area,
    pub rows: Vec<Row>,
    /// Store revision at snapshot time.
    pub revision: u64,
}

/// Take a snapshot of one area under a read lock.
pub fn snapshot_area(store: &RegisterStore, area: Area) -> AreaSnapshot {
    let cells = &store.cells[area.index()];
    let rows = (0..cells.len())
        .map(|i| Row {
            addr: i,
            name: cells[i].name.clone(),
            value: cells[i].value,
            writer: cells[i].writer.clone(),
            changed: false, // caller compares with the previous snapshot
        })
        .collect();
    AreaSnapshot {
        area,
        rows,
        revision: store.revision,
    }
}

/// Shared handle used by protocol servers and the UI.
pub type SharedStore = Arc<RwLock<RegisterStore>>;

/// Convenience: shared store with the default area size.
pub fn shared_store(size: usize) -> SharedStore {
    Arc::new(RwLock::new(RegisterStore::new(size)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A known 32-bit value with four distinct bytes: 0x12 0x34 0x56 0x78.
    const V: u32 = 0x1234_5678;

    #[test]
    fn byte_order_abcd_encode() {
        assert_eq!(ByteOrder::Abcd.encode_u32(V), [0x1234, 0x5678]);
    }

    #[test]
    fn byte_order_cdab_encode() {
        // Register0 = low word first.
        assert_eq!(ByteOrder::Cdab.encode_u32(V), [0x5678, 0x1234]);
    }

    #[test]
    fn byte_order_badc_encode() {
        // Word-swap within each word: 0x1234 -> 0x3412, 0x5678 -> 0x7856.
        assert_eq!(ByteOrder::Badc.encode_u32(V), [0x3412, 0x7856]);
    }

    #[test]
    fn byte_order_dcba_encode() {
        // Byte + word swap: [0x1234] -> [0x7856], [0x5678] -> [0x3412].
        assert_eq!(ByteOrder::Dcba.encode_u32(V), [0x7856, 0x3412]);
    }

    #[test]
    fn byte_order_round_trip_all_orders() {
        for order in ByteOrder::ALL {
            let words = order.encode_u32(V);
            assert_eq!(
                order.decode_u32(words),
                V,
                "round-trip failed for {:?}",
                order
            );
        }
    }

    #[test]
    fn byte_order_exact_wire_patterns() {
        // ABCD should read back as Big-Endian across the two registers.
        assert_eq!(ByteOrder::Abcd.decode_u32([0x1234, 0x5678]), 0x1234_5678);
        // CDAB: register order swapped.
        assert_eq!(ByteOrder::Cdab.decode_u32([0x5678, 0x1234]), 0x1234_5678);
        // BADC: each register byte-swapped.
        assert_eq!(ByteOrder::Badc.decode_u32([0x3412, 0x7856]), 0x1234_5678);
        // DCBA: byte + word swapped.
        assert_eq!(ByteOrder::Dcba.decode_u32([0x7856, 0x3412]), 0x1234_5678);
    }

    #[test]
    fn data_width_registers_and_bits() {
        assert_eq!(DataWidth::Bits16.registers(), 1);
        assert_eq!(DataWidth::Bits16.bits(), 16);
        assert_eq!(DataWidth::Bits32.registers(), 2);
        assert_eq!(DataWidth::Bits32.bits(), 32);
    }

    #[test]
    fn value_type_fixed_registers() {
        assert_eq!(ValueType::Uint16.fixed_registers(), Some(1));
        assert_eq!(ValueType::Int16.fixed_registers(), Some(1));
        assert_eq!(ValueType::Uint32.fixed_registers(), Some(2));
        assert_eq!(ValueType::Float32.fixed_registers(), Some(2));
        assert_eq!(ValueType::String.fixed_registers(), None);
    }

    #[test]
    fn value_type_uint16_round_trip() {
        let words = ValueType::Uint16
            .encode_text("12345", ByteOrder::Abcd, 1)
            .unwrap();
        assert_eq!(words, vec![12345]);
        assert_eq!(
            ValueType::Uint16.decode_words(&words, ByteOrder::Abcd),
            "12345"
        );
    }

    #[test]
    fn value_type_int32_round_trip_with_byte_order() {
        for bo in ByteOrder::ALL {
            let words = ValueType::Int32.encode_text("-123456789", bo, 2).unwrap();
            assert_eq!(words.len(), 2);
            assert_eq!(
                ValueType::Int32.decode_words(&words, bo),
                "-123456789",
                "byte order {:?}",
                bo
            );
        }
    }

    #[test]
    fn value_type_float32_round_trip() {
        let words = ValueType::Float32
            .encode_text("2.5", ByteOrder::Abcd, 2)
            .unwrap();
        let decoded = ValueType::Float32
            .decode_words(&words, ByteOrder::Abcd)
            .parse::<f32>()
            .unwrap();
        assert!((decoded - 2.5).abs() < 1e-4);
    }

    #[test]
    fn value_type_string_auto_fill_and_char() {
        // "A" -> single char; with pad -> 1 register holding "A\0" i.e. 0x4100.
        let words = ValueType::String
            .encode_text("A", ByteOrder::Abcd, 8)
            .unwrap();
        assert_eq!(words.len(), 1);
        assert_eq!(words[0], 0x4100);
        assert_eq!(ValueType::String.decode_words(&words, ByteOrder::Abcd), "A");

        // UTF-8 two bytes "中" (E4 B8 AD) -> with pad -> 2 registers.
        let words = ValueType::String
            .encode_text("中", ByteOrder::Abcd, 8)
            .unwrap();
        assert_eq!(words.len(), 2);
        assert_eq!(
            ValueType::String.decode_words(&words, ByteOrder::Abcd),
            "中"
        );

        // Multi-char ASCII string.
        let words = ValueType::String
            .encode_text("Hi", ByteOrder::Abcd, 8)
            .unwrap();
        assert_eq!(words, vec![0x4869]);
        assert_eq!(
            ValueType::String.decode_words(&words, ByteOrder::Abcd),
            "Hi"
        );
    }

    #[test]
    fn value_type_string_too_long_rejected() {
        assert!(ValueType::String
            .encode_text("0123456789abcdef", ByteOrder::Abcd, 4)
            .is_err());
    }

    #[test]
    fn value_type_invalid_number_rejected() {
        assert!(ValueType::Uint16
            .encode_text("abc", ByteOrder::Abcd, 1)
            .is_err());
        assert!(ValueType::Float32
            .encode_text("12.5x", ByteOrder::Abcd, 2)
            .is_err());
    }

    #[test]
    fn bytes_to_regs_rounds_up() {
        assert_eq!(bytes_to_regs(1), 1);
        assert_eq!(bytes_to_regs(2), 1);
        assert_eq!(bytes_to_regs(7), 4);
        assert_eq!(bytes_to_regs(8), 4);
        assert_eq!(bytes_to_regs(5), 3);
    }

    #[test]
    fn encode_string_fixed_pads_to_width() {
        // String(7) -> 7 bytes -> 4 registers, short "Hi" NUL-padded.
        let words = encode_string_fixed("Hi", ByteOrder::Abcd, 8, 7).unwrap();
        assert_eq!(words.len(), 4);
        assert_eq!(words, vec![0x4869, 0x0000, 0x0000, 0x0000]);
        assert_eq!(
            ValueType::String.decode_words(&words, ByteOrder::Abcd),
            "Hi"
        );
    }

    #[test]
    fn encode_string_fixed_rejects_overflow() {
        // "HelloWorld" is 10 bytes > 7-byte buffer.
        assert!(encode_string_fixed("HelloWorld", ByteOrder::Abcd, 8, 7).is_err());
        // 8 registers needed for String(16); budget is 4 -> rejected.
        assert!(encode_string_fixed("abc", ByteOrder::Abcd, 4, 16).is_err());
    }

    #[test]
    fn encode_string_fixed_respects_byte_order() {
        for bo in ByteOrder::ALL {
            let words = encode_string_fixed("AB", bo, 8, 2).unwrap();
            assert_eq!(words.len(), 1);
            assert_eq!(ValueType::String.decode_words(&words, bo), "AB");
        }
    }

    #[test]
    fn byte_order_u64_round_trip_all_orders() {
        let v: u64 = 0x0123_4567_89AB_CDEF;
        for bo in ByteOrder::ALL {
            let words = bo.encode_u64(v);
            assert_eq!(bo.decode_u64(words), v, "round-trip failed for {:?}", bo);
        }
    }

    #[test]
    fn byte_order_u64_exact_wire_pattern() {
        // ABCD: high u32 in registers 0..2, low u32 in 2..4.
        let v: u64 = 0x1234_5678_9ABC_DEF0;
        assert_eq!(
            ByteOrder::Abcd.encode_u64(v),
            [0x1234, 0x5678, 0x9ABC, 0xDEF0]
        );
        // CDAB: each 32-bit half word-swapped; high half first.
        assert_eq!(
            ByteOrder::Cdab.encode_u64(v),
            [0x5678, 0x1234, 0xDEF0, 0x9ABC]
        );
    }

    #[test]
    fn value_type_64bit_round_trip() {
        for bo in ByteOrder::ALL {
            // UInt64
            let words = ValueType::Uint64
                .encode_text("123456789012345", bo, 4)
                .unwrap();
            assert_eq!(
                ValueType::Uint64.decode_words(&words, bo),
                "123456789012345",
                "{:?}",
                bo
            );
            // Int64 (negative)
            let words = ValueType::Int64
                .encode_text("-98765432109876543", bo, 4)
                .unwrap();
            assert_eq!(
                ValueType::Int64.decode_words(&words, bo),
                "-98765432109876543",
                "{:?}",
                bo
            );
            // Double
            let words = ValueType::Double.encode_text("3.14159", bo, 4).unwrap();
            let decoded = ValueType::Double
                .decode_words(&words, bo)
                .parse::<f64>()
                .unwrap();
            assert!((decoded - 3.14159).abs() < 1e-5, "{:?}", bo);
        }
    }

    #[test]
    fn value_type_64bit_register_width() {
        assert_eq!(ValueType::Uint64.fixed_registers(), Some(4));
        assert_eq!(ValueType::Int64.fixed_registers(), Some(4));
        assert_eq!(ValueType::Double.fixed_registers(), Some(4));
        assert_eq!(ValueType::Uint64.bits(), Some(64));
        assert_eq!(ValueType::Int64.bits(), Some(64));
        assert_eq!(ValueType::Double.bits(), Some(64));
    }
}
