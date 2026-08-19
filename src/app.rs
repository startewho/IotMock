//! Main application view: title bar, protocol sidebar, register table, dialogs.

use std::rc::Rc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use gpui::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, AsyncApp, Context, Entity,
    InteractiveElement, IntoElement, ParentElement as _, Render, Styled as _, Subscription, Task,
    Window, WindowControlArea,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputState},
    notification::Notification,
    scroll::ScrollableElement as _,
    select::{Select, SelectItem, SelectState},
    switch::Switch,
    tab::{Tab, TabBar},
    table::{Column, Table, TableDelegate, TableEvent, TableState},
    v_flex, ActiveTheme as _, Disableable, Icon, IconName, IndexPath, Root, Sizable as _, Theme,
    ThemeMode, WindowExt as _,
};

use iot_mock::model::{
    bytes_to_regs, encode_string_fixed, shared_store, snapshot_area, Area, AreaSnapshot, ByteOrder,
    RegisterStore, Row, SharedStore, ValueType, ALL_AREAS, DEFAULT_AREA_SIZE,
};
use iot_mock::protocol::{
    modbus::{parse_modbus_hex, ModbusFrameParse, ModbusTcpServer, DEFAULT_PORT},
    ProtocolCard, ProtocolContext, ServerState, ServerStats,
};

/// UI refresh interval (milliseconds).
const REFRESH_MS: u64 = 200;

/// A local newtype so [`ByteOrder`] (an external type) can be a [`SelectItem`]
/// (an external trait) without violating orphan rules.
#[derive(Clone, Copy)]
struct ByteOrderItem(ByteOrder);

impl SelectItem for ByteOrderItem {
    type Value = ByteOrder;

    fn title(&self) -> gpui::SharedString {
        self.0.name().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

impl From<ByteOrder> for ByteOrderItem {
    fn from(b: ByteOrder) -> Self {
        Self(b)
    }
}

/// A local newtype so [`ValueType`] (external) can be a [`SelectItem`].
#[derive(Clone, Copy)]
struct ValueTypeItem(ValueType);

impl SelectItem for ValueTypeItem {
    type Value = ValueType;

    fn title(&self) -> gpui::SharedString {
        self.0.label().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

impl From<ValueType> for ValueTypeItem {
    fn from(v: ValueType) -> Self {
        Self(v)
    }
}

/// Parse a space-separated list of 16-bit hex values such as `"1234 5678"`
/// (optional `0x` prefixes are accepted) into register words.
fn parse_hex_words(text: &str) -> Result<Vec<u16>, String> {
    let tokens: Vec<&str> = text.split_whitespace().filter(|t| !t.is_empty()).collect();
    if tokens.is_empty() {
        return Err("请输入十六进制寄存器值".to_string());
    }
    let mut words = Vec::with_capacity(tokens.len());
    for t in tokens {
        let cleaned = t
            .strip_prefix("0x")
            .or_else(|| t.strip_prefix("0X"))
            .unwrap_or(t);
        let v = u16::from_str_radix(cleaned, 16).map_err(|_| format!("无效的十六进制 '{t}'"))?;
        words.push(v);
    }
    Ok(words)
}

/// The register table delegate: columns + per-area row rendering.
pub struct RegTableDelegate {
    area: Area,
    rows: Vec<Row>,
    prev: Vec<u16>,
    columns: Vec<Column>,
    /// Shared register store, used by the in-table bit checkboxes to read the
    /// live value and write immediately on toggle.
    store: SharedStore,
}

impl RegTableDelegate {
    fn new(store: SharedStore) -> Self {
        Self {
            area: Area::HoldingRegisters,
            rows: Vec::new(),
            prev: Vec::new(),
            store,
            columns: vec![
                Column::new("addr", "地址").width(80.).resizable(false),
                Column::new("name", "名称").width(150.).resizable(false),
                Column::new("bits", "位 (Bit)").width(240.).resizable(true),
                Column::new("value", "值").width(140.).resizable(false),
                Column::new("writer", "最后写入")
                    .width(130.)
                    .resizable(false),
            ],
        }
    }

    /// Replace the displayed rows with a fresh snapshot, flagging rows whose
    /// value changed since the previous snapshot.
    fn apply_snapshot(&mut self, snap: AreaSnapshot) {
        let old = std::mem::take(&mut self.prev);
        let mut rows: Vec<Row> = snap.rows;
        for (i, row) in rows.iter_mut().enumerate() {
            row.changed = old.get(i).is_some_and(|&p| p != row.value);
        }
        self.prev = rows.iter().map(|r| r.value).collect();
        self.area = snap.area;
        self.rows = rows;
    }

    /// Switch to another area; the next snapshot repopulates the rows.
    #[allow(dead_code)]
    fn set_area(&mut self, area: Area) {
        self.area = area;
        self.rows.clear();
        self.prev.clear();
    }

    /// Immediately reflect a UI-driven write in the table (no wait for poll).
    fn poke_row(&mut self, addr: usize, value: u16, writer: &str) {
        if let Some(row) = self.rows.iter_mut().find(|r| r.addr == addr) {
            row.value = value;
            row.writer = writer.to_string();
            row.changed = true;
        }
        if let Some(p) = self.prev.get_mut(addr) {
            *p = value;
        }
    }
}

impl TableDelegate for RegTableDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> &Column {
        &self.columns[col_ix]
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let Some(row) = self.rows.get(row_ix) else {
            return div().into_any_element();
        };
        let bit = self.area.is_bit();
        let changed = row.changed;

        let el = match self.columns[col_ix].key.as_ref() {
            "addr" => div()
                .px_2()
                .text_color(theme.muted_foreground)
                .child(format!("{0}", row.addr)),
            "name" => div().px_2().child(row.name.clone()),
            "bits" => {
                // Read the live value so an in-place toggle reflects instantly.
                let v = self
                    .store
                    .read()
                    .unwrap()
                    .get(self.area, row.addr)
                    .unwrap_or(0);
                let writable = self.area.writable();
                let store = self.store.clone();
                let area = self.area;
                let addr = row.addr;
                let mk_bit = move |bit_ix: usize, on: bool| -> gpui::AnyElement {
                    let store = store.clone();
                    Checkbox::new(gpui::SharedString::from(format!("tblbit-{addr}-{bit_ix}")))
                        .small()
                        .checked(on)
                        .disabled(!writable)
                        .on_click(move |&checked, window, _cx| {
                            let mut s = store.write().unwrap();
                            let cur = s.get(area, addr).unwrap_or(0);
                            let newv = if checked {
                                cur | (1 << bit_ix)
                            } else {
                                cur & !(1 << bit_ix)
                            };
                            s.set(area, addr, newv, "UI");
                            drop(s);
                            // Force an immediate redraw so the checkbox flick
                            // (the live-value read above picks it up).
                            window.refresh();
                        })
                        .into_any_element()
                };
                if bit {
                    h_flex()
                        .px_2()
                        .gap_1()
                        .items_center()
                        .child(mk_bit(0, v & 1 == 1))
                } else {
                    v_flex()
                        .px_2()
                        .gap_0()
                        .child(
                            h_flex()
                                .gap_1()
                                .children((0..8).map(|i| mk_bit(i, v >> i & 1 == 1))),
                        )
                        .child(
                            h_flex()
                                .gap_1()
                                .children((8..16).map(|i| mk_bit(i, v >> i & 1 == 1))),
                        )
                }
            }
            "value" => {
                let color = if bit {
                    if row.value != 0 {
                        theme.green
                    } else {
                        theme.muted_foreground
                    }
                } else if changed {
                    theme.yellow
                } else {
                    theme.foreground
                };
                let text = if bit {
                    row.value.to_string()
                } else {
                    format!("{:#06X}  ({})", row.value, row.value)
                };
                h_flex()
                    .px_2()
                    .gap_1()
                    .items_center()
                    .when(changed, |this| {
                        this.child(
                            div()
                                .rounded_full()
                                .px_1()
                                .text_size(px(12.))
                                .text_color(theme.foreground)
                                .bg(theme.yellow.opacity(0.15))
                                .child("changed"),
                        )
                    })
                    .child(div().text_color(color).child(text))
            }
            "writer" => div()
                .px_2()
                .text_color(theme.muted_foreground)
                .child(row.writer.clone()),
            _ => div(),
        };

        // Tint the whole cell when the value just changed.
        el.when(changed, |this| this.bg(theme.primary.opacity(0.06)))
            .into_any_element()
    }
}

/// How the edit dialog presents the raw register value.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ViewMode {
    /// Toggle individual bits with checkboxes.
    #[default]
    Bits,
    /// Edit the raw register words as 16-bit hexadecimal values.
    Hex,
}

/// Editor state for the register bit/typed/hex edit dialog.
///
/// Kept as its own mutable entity because the dialog's render function is
/// re-invoked every frame; the checkboxes and inputs read/write this entity so
/// their state survives re-renders. `bits` holds every bit of the value (LSB
/// of register 0 first); its length is `reg_count * 16`.
pub struct BitEditorState {
    /// Raw bits of the value (`reg_count * 16` entries).
    bits: Vec<bool>,
    /// Number of 16-bit registers spanned by the current value.
    reg_count: usize,
    addr: usize,
    area: Area,
    byte_order: ByteOrder,
    /// The type used to interpret and "auto-fill" the registers.
    value_type: ValueType,
    /// `String(N)` byte capacity (only meaningful when `value_type == String`).
    string_chars: usize,
    /// Last accepted string for `ValueType::String` (may be many registers).
    pending_string: String,
    view_mode: ViewMode,
}

impl BitEditorState {
    fn new() -> Self {
        Self {
            bits: vec![false; 16],
            reg_count: 1,
            addr: 0,
            area: Area::HoldingRegisters,
            byte_order: ByteOrder::Abcd,
            value_type: ValueType::Uint16,
            string_chars: 7,
            pending_string: String::new(),
            view_mode: ViewMode::Bits,
        }
    }

    /// Register count spanned by the current value: a fixed width for numeric
    /// types, or `bytes_to_regs(string_chars)` for strings.
    fn regs_for_type(&self) -> usize {
        match self.value_type {
            ValueType::String => bytes_to_regs(self.string_chars),
            t => t.fixed_registers().unwrap_or(1),
        }
    }

    /// Byte length of the current value: `string_chars` for strings, otherwise
    /// derived from the register count.
    pub fn byte_len(&self) -> usize {
        match self.value_type {
            ValueType::String => self.string_chars,
            _ => self.reg_count * 2,
        }
    }

    /// Load the current value at `addr` into the raw bit view, reading
    /// `reg_count` registers (`bytes_to_regs(string_chars)` for strings).
    fn load(&mut self, store: &RegisterStore, byte_order: ByteOrder) -> bool {
        let reg_count = self.regs_for_type();
        let words = store
            .range(self.area, self.addr, reg_count)
            .unwrap_or_else(|| vec![0; reg_count]);
        self.byte_order = byte_order;
        self.reg_count = words.len();
        self.set_words(&words);
        true
    }

    /// Overwrite the raw bit view from register words: register `k` occupies
    /// bits `[k*16, (k+1)*16)`, bit 0 of each register being its LSB.
    fn set_words(&mut self, words: &[u16]) {
        self.reg_count = words.len();
        self.bits = vec![false; words.len() * 16];
        for (k, &w) in words.iter().enumerate() {
            for i in 0..16 {
                self.bits[k * 16 + i] = (w >> i) & 1 == 1;
            }
        }
    }

    /// Assemble the raw register words from the current bits.
    fn words(&self) -> Vec<u16> {
        (0..self.reg_count)
            .map(|k| {
                let mut w = 0u16;
                for i in 0..16 {
                    if self.bits.get(k * 16 + i).copied().unwrap_or(false) {
                        w |= 1 << i;
                    }
                }
                w
            })
            .collect()
    }

    fn set_bit(&mut self, ix: usize, on: bool) {
        if let Some(b) = self.bits.get_mut(ix) {
            *b = on;
        }
    }

    /// Apply a typed value (number or string) from `text` into the store at
    /// `addr`, then re-sync the raw bit view. On success returns the words that
    /// were written; on parse/range failure returns `Err(message)`.
    fn apply_typed(&mut self, store: &mut RegisterStore, text: &str) -> Result<Vec<u16>, String> {
        if !self.area.writable() {
            return Err("区域只读".to_string());
        }
        let budget = store.len(self.area).saturating_sub(self.addr);
        let words = if self.value_type == ValueType::String {
            encode_string_fixed(text, self.byte_order, budget, self.string_chars)?
        } else {
            self.value_type.encode_text(text, self.byte_order, budget)?
        };
        if self.value_type == ValueType::String {
            self.pending_string = text.to_string();
        }
        self.set_words(&words);
        if !store.set_range(self.area, self.addr, &words, "UI") {
            return Err("地址超出范围".to_string());
        }
        Ok(words)
    }

    /// Write the current raw words (from bit or hex editing) to the store.
    fn apply_words(&mut self, store: &mut RegisterStore) -> Result<Vec<u16>, String> {
        if !self.area.writable() {
            return Err("区域只读".to_string());
        }
        let words = self.words();
        if words.is_empty() {
            return Err("没有可写入的数据".to_string());
        }
        if !store.set_range(self.area, self.addr, &words, "UI") {
            return Err("地址超出范围".to_string());
        }
        Ok(words)
    }

    /// Human-readable rendering of the currently active value (for the dialog
    /// header/preview).
    fn display_value(&self, store: &RegisterStore) -> String {
        let words = store
            .range(self.area, self.addr, self.reg_count)
            .unwrap_or_default();
        self.value_type.decode_words(&words, self.byte_order)
    }

    /// Render the current in-memory raw words as a typed string (kept in sync
    /// with the typed input each time a checkbox is toggled).
    fn bits_text(&self) -> String {
        let words = self.words();
        self.value_type.decode_words(&words, self.byte_order)
    }

    /// Render the current raw words as space-separated 16-bit hex, e.g.
    /// `"1234 5678"`.
    fn hex_text(&self) -> String {
        self.words()
            .iter()
            .map(|w| format!("{w:04X}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Number of registers to read/write for the current type/dialog view.
    fn registers(&self) -> usize {
        self.reg_count
    }
}

/// State for the Modbus hex-parse / decode dialog.
///
/// Similar to [`BitEditorState`] but purely a viewer/decoder: it holds the
/// register words extracted from the frame, lets the user pick a type and byte
/// order, and renders the decoded value plus a bit-wise grid. It never writes
/// to the shared store.
pub struct ParserState {
    /// The active words used for interpretation (length = `type_word_count()`,
    /// padded/truncated to the type's width).
    words: Vec<u16>,
    /// Bit view of `words` (`len * 16` entries), LSB of word 0 first.
    bits: Vec<bool>,
    value_type: ValueType,
    byte_order: ByteOrder,
    /// `String(N)` byte capacity (only meaningful for `String`).
    string_chars: usize,
    /// Last successful parse result (function code + note), if any.
    info: Option<ModbusFrameParse>,
    /// Last parse / decode error message, if any.
    error: Option<String>,
}

impl ParserState {
    fn new() -> Self {
        Self {
            words: vec![0],
            bits: vec![false; 16],
            value_type: ValueType::Uint16,
            byte_order: ByteOrder::Abcd,
            string_chars: 7,
            info: None,
            error: None,
        }
    }

    /// Number of 16-bit words the current type spans.
    fn type_word_count(&self) -> usize {
        match self.value_type {
            ValueType::String => bytes_to_regs(self.string_chars),
            t => t.fixed_registers().unwrap_or(1),
        }
    }

    /// Rebuild the active words (resized to the type's width) and the bit view.
    fn refresh(&mut self) {
        let n = self.type_word_count();
        if self.words.len() < n {
            self.words.resize(n, 0);
        }
        self.words.truncate(n);
        self.bits = vec![false; n * 16];
        for (k, &w) in self.words.iter().enumerate() {
            for i in 0..16 {
                self.bits[k * 16 + i] = (w >> i) & 1 == 1;
            }
        }
    }

    /// Adopt the register words parsed from a frame.
    fn set_data(&mut self, words: Vec<u16>) {
        self.words = words;
        self.refresh();
    }

    /// Decode the active words under the current type + byte order.
    fn decoded(&self) -> String {
        self.value_type.decode_words(&self.words, self.byte_order)
    }

    /// Space-separated hex of the active words.
    fn hex_text(&self) -> String {
        self.words
            .iter()
            .map(|w| format!("{w:04X}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Toggle a single bit and rebuild the words from the bit view.
    fn toggle_bit(&mut self, ix: usize, on: bool) {
        if let Some(b) = self.bits.get_mut(ix) {
            *b = on;
        }
        let n = self.type_word_count();
        self.words = (0..n)
            .map(|k| {
                let mut w = 0u16;
                for i in 0..16 {
                    if self.bits[k * 16 + i] {
                        w |= 1 << i;
                    }
                }
                w
            })
            .collect();
    }

    fn set_type(&mut self, vt: ValueType) {
        self.value_type = vt;
        self.refresh();
    }

    fn set_byte_order(&mut self, bo: ByteOrder) {
        self.byte_order = bo;
    }

    fn set_string_chars(&mut self, chars: usize) {
        self.string_chars = chars.clamp(1, 120);
        self.refresh();
    }
}

/// The top-level application view.
pub struct AppView {
    store: SharedStore,
    stats: Arc<ServerStats>,
    protocols: Vec<ProtocolCard>,
    port_inputs: Vec<Entity<InputState>>,
    /// One byte-order dropdown per protocol, applied when the server starts.
    byte_order_selects: Vec<Entity<SelectState<Vec<ByteOrderItem>>>>,
    /// Shared state for the bit/typed edit dialog.
    bit_editor: Entity<BitEditorState>,
    /// Type dropdown in the edit dialog.
    type_select: Entity<SelectState<Vec<ValueTypeItem>>>,
    /// Numeric / text inputs reused by the edit dialog.
    num_input: Entity<InputState>,
    str_input: Entity<InputState>,
    /// Hexadecimal raw-word editor (space-separated `XXXX`) in the dialog.
    hex_input: Entity<InputState>,
    /// `String(N)` byte-length input in the dialog.
    str_len_input: Entity<InputState>,
    /// Parser dialog state (Modbus hex decode).
    parser_state: Entity<ParserState>,
    /// Type dropdown in the parser dialog.
    parser_type_select: Entity<SelectState<Vec<ValueTypeItem>>>,
    /// Byte-order dropdown in the parser dialog.
    parser_byte_order_select: Entity<SelectState<Vec<ByteOrderItem>>>,
    /// The raw Modbus frame hex input in the parser dialog.
    parser_hex_input: Entity<InputState>,
    /// `String(N)` byte-length input in the parser dialog.
    parser_str_len_input: Entity<InputState>,
    table: Entity<TableState<RegTableDelegate>>,
    active_area: Area,
    selected_row: Option<usize>,
    auto_sim: Arc<AtomicBool>,
    last_revision: u64,
    _refresh_task: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = shared_store(DEFAULT_AREA_SIZE);
        let stats = Arc::new(ServerStats::default());

        // -- protocol servers -------------------------------------------------
        // Extension point: add more `ProtocolCard`s here (e.g. S7, OPC-UA...).
        let protocols = vec![ProtocolCard::new(Box::new(ModbusTcpServer::new(
            DEFAULT_PORT,
        )))];

        let port_inputs = protocols
            .iter()
            .map(|card| {
                let port = card.protocol.port().to_string();
                cx.new(|cx| InputState::new(window, cx).default_value(port))
            })
            .collect::<Vec<_>>();

        // One byte-order dropdown per protocol (default: ABCD).
        let byte_order_selects = (0..protocols.len())
            .map(|_| {
                let items = ByteOrder::ALL
                    .iter()
                    .map(|&b| ByteOrderItem(b))
                    .collect::<Vec<_>>();
                cx.new(|cx| SelectState::new(items, Some(IndexPath::default().row(0)), window, cx))
            })
            .collect::<Vec<_>>();

        // -- register table ---------------------------------------------------
        let table = cx.new(|cx| {
            TableState::new(RegTableDelegate::new(store.clone()), window, cx)
                .sortable(false)
                .col_movable(false)
                .col_resizable(true)
                .row_selectable(true)
                .col_selectable(false)
        });

        // Shared state for the bit/typed edit dialog.
        let bit_editor = cx.new(|_| BitEditorState::new());

        // Type dropdown + reusable inputs for the edit dialog.
        let type_items = ValueType::ALL
            .iter()
            .map(|&v| ValueTypeItem(v))
            .collect::<Vec<_>>();
        let type_select = cx.new(|cx| {
            SelectState::new(
                type_items,
                Some(IndexPath::default().row(0)), // Uint16 default
                window,
                cx,
            )
        });
        let num_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("输入数值，回车或点应用"));
        let str_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("输入字符串 / 字符，回车或点应用"));
        let hex_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("十六进制寄存器，如 1234 5678"));
        let str_len_input = cx.new(|cx| InputState::new(window, cx).default_value("7"));

        // Parser dialog: hex input, type/byte-order dropdowns, string length.
        let parser_state = cx.new(|_| ParserState::new());
        let parser_type_items = ValueType::ALL
            .iter()
            .map(|&v| ValueTypeItem(v))
            .collect::<Vec<_>>();
        let parser_type_select = cx.new(|cx| {
            SelectState::new(
                parser_type_items,
                Some(IndexPath::default().row(0)), // Uint16 default
                window,
                cx,
            )
        });
        let parser_order_items = ByteOrder::ALL
            .iter()
            .map(|&b| ByteOrderItem(b))
            .collect::<Vec<_>>();
        let parser_byte_order_select = cx.new(|cx| {
            SelectState::new(
                parser_order_items,
                Some(IndexPath::default().row(0)), // ABCD default
                window,
                cx,
            )
        });
        let parser_hex_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("粘贴 Modbus TCP 帧十六进制，如 0001 0000 0005 01 03 02 12 34")
                .default_value("0001 0000 0005 01 03 02 12 34")
        });
        let parser_str_len_input = cx.new(|cx| InputState::new(window, cx).default_value("7"));
        // Pre-parse the default example right away.
        {
            let hex = parser_hex_input.read(cx).value().to_string();
            match parse_modbus_hex(&hex) {
                Ok(info) => {
                    parser_state.update(cx, |s, _| {
                        s.set_data(info.words.clone());
                        s.info = Some(info);
                        s.error = None;
                    });
                }
                Err(e) => parser_state.update(cx, |s, _| s.error = Some(e)),
            }
        }

        let auto_sim = Arc::new(AtomicBool::new(true));

        let mut subscriptions = Vec::new();
        subscriptions.push(
            cx.subscribe_in(
                &table,
                window,
                |this, _table, event, window, cx| match event {
                    TableEvent::DoubleClickedRow(ix) => {
                        this.open_edit_dialog(*ix, window, cx);
                    }
                    TableEvent::SelectRow(ix) => {
                        this.selected_row = Some(*ix);
                        cx.notify();
                    }
                    _ => {}
                },
            ),
        );

        // When the value-type dropdown changes, update the editor's type and
        // refresh the typed-input preview.
        subscriptions.push(cx.subscribe_in(
            &type_select,
            window,
            |this, _sel, event, window, cx| {
                if let gpui_component::select::SelectEvent::Confirm(Some(vt)) = event {
                    this.on_value_type_changed(*vt, window, cx);
                }
            },
        ));

        // When the `String(N)` length changes, resize the raw view and reload.
        subscriptions.push(cx.subscribe_in(
            &str_len_input,
            window,
            |this, _sel, event, window, cx| {
                if let gpui_component::input::InputEvent::Change = event {
                    this.sync_string_len_from_input(window, cx);
                    let vt = this.bit_editor.read(cx).value_type;
                    this.sync_dialog_inputs(vt, window, cx);
                    cx.notify();
                }
            },
        ));

        // Parser: re-parse the frame when the hex input changes.
        subscriptions.push(cx.subscribe_in(
            &parser_hex_input,
            window,
            |this, _inp, event, window, cx| {
                if let gpui_component::input::InputEvent::Change = event {
                    this.parse_hex_into_parser(window, cx);
                    window.refresh();
                }
            },
        ));
        // Parser: value-type change.
        subscriptions.push(cx.subscribe_in(
            &parser_type_select,
            window,
            |this, _sel, event, window, cx| {
                if let gpui_component::select::SelectEvent::Confirm(Some(vt)) = event {
                    this.parser_state.update(cx, |s, _| s.set_type(*vt));
                    this.parser_sync_inputs(window, cx);
                    window.refresh();
                }
            },
        ));
        // Parser: byte-order change.
        subscriptions.push(cx.subscribe_in(
            &parser_byte_order_select,
            window,
            |this, _sel, event, window, cx| {
                if let gpui_component::select::SelectEvent::Confirm(Some(bo)) = event {
                    this.parser_state.update(cx, |s, _| s.set_byte_order(*bo));
                    window.refresh();
                }
            },
        ));
        // Parser: `String(N)` length change.
        subscriptions.push(cx.subscribe_in(
            &parser_str_len_input,
            window,
            |this, _sel, event, window, cx| {
                if let gpui_component::input::InputEvent::Change = event {
                    let text = this.parser_str_len_input.read(cx).value().to_string();
                    let n = text.trim().parse::<usize>().unwrap_or(7);
                    this.parser_state.update(cx, |s, _| s.set_string_chars(n));
                    this.parser_sync_inputs(window, cx);
                    window.refresh();
                }
            },
        ));

        // -- real-time refresh loop -------------------------------------------
        // Every REFRESH_MS: (optionally) auto-simulate, then push a fresh
        // snapshot of the active area into the table delegate.
        let store_loop = store.clone();
        let _refresh_task = cx.spawn(async move |this, cx: &mut AsyncApp| {
            let mut tick: u64 = 0;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(REFRESH_MS))
                    .await;

                let done = this
                    .update(cx, |app, cx| {
                        if app.auto_sim.load(Ordering::Relaxed) {
                            store_loop.write().unwrap().simulate_tick(tick);
                        }
                        app.tick_ui(cx);
                    })
                    .is_err();
                if done {
                    break; // the view was released
                }
                tick = tick.wrapping_add(1);
            }
        });

        let mut this = Self {
            store,
            stats,
            protocols,
            port_inputs,
            byte_order_selects,
            bit_editor,
            type_select,
            num_input,
            str_input,
            hex_input,
            str_len_input,
            parser_state,
            parser_type_select,
            parser_byte_order_select,
            parser_hex_input,
            parser_str_len_input,
            table,
            active_area: Area::HoldingRegisters,
            selected_row: None,
            auto_sim,
            last_revision: 0,
            _refresh_task,
            _subscriptions: subscriptions,
        };

        // Initial snapshot so the table is populated immediately.
        let snap = snapshot_area(&this.store.read().unwrap(), this.active_area);
        this.last_revision = snap.revision;
        this.table.update(cx, |t, cx| {
            t.delegate_mut().apply_snapshot(snap);
            cx.notify();
        });

        this
    }

    // -----------------------------------------------------------------------
    // Refresh / data helpers
    // -----------------------------------------------------------------------

    /// Called from the refresh loop: snapshot the active area into the table
    /// and update the cached revision.
    fn tick_ui(&mut self, cx: &mut Context<Self>) {
        self.push_snapshot(cx);
    }

    fn push_snapshot(&mut self, cx: &mut Context<Self>) {
        let snap = {
            let guard = self.store.read().unwrap();
            snapshot_area(&guard, self.active_area)
        };
        self.last_revision = snap.revision;
        self.table.update(cx, |t, cx| {
            t.delegate_mut().apply_snapshot(snap);
            cx.notify();
        });
        cx.notify();
    }

    fn set_active_area(&mut self, area: Area, _window: &mut Window, cx: &mut Context<Self>) {
        if self.active_area == area {
            return;
        }
        self.active_area = area;
        self.selected_row = None;
        self.push_snapshot(cx);
    }

    // -----------------------------------------------------------------------
    // Actions
    // -----------------------------------------------------------------------

    fn toggle_protocol(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let running = self
            .protocols
            .get(ix)
            .map(|c| c.protocol.is_running())
            .unwrap_or(false);

        if running {
            if let Some(card) = self.protocols.get_mut(ix) {
                card.protocol.stop();
            }
            window.push_notification(
                Notification::info(format!("{} 已停止", self.protocols[ix].protocol.name())),
                cx,
            );
        } else {
            self.start_protocol(ix, window, cx);
        }
        cx.notify();
    }

    fn start_protocol(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        // Read config first (immutable borrows of self), then take the mutable
        // borrow of the protocol card to avoid overlapping borrows.
        let port_text = self.port_inputs[ix].read(cx).value().to_string();
        let port = port_text.trim().parse::<u16>().unwrap_or(502);
        let byte_order = self.selected_byte_order(ix, cx);

        let Some(card) = self.protocols.get_mut(ix) else {
            return;
        };
        card.protocol.set_port(port);

        let ctx = ProtocolContext {
            store: self.store.clone(),
            stats: self.stats.clone(),
        };

        match card.protocol.start(&ctx) {
            Ok(()) => {
                window.push_notification(
                    Notification::success(format!(
                        "{} 已启动，监听端口 {} · 字节序 {}",
                        card.protocol.name(),
                        port,
                        byte_order.code()
                    )),
                    cx,
                );
                log::info!(
                    "[ui] {} started on port {port}, byte order {}",
                    card.protocol.name(),
                    byte_order.code()
                );
            }
            Err(e) => {
                window.push_notification(
                    Notification::error(format!("{} 启动失败: {e}", card.protocol.name())),
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// The byte order currently selected in the dropdown for protocol `ix`.
    fn selected_byte_order(&self, ix: usize, cx: &Context<Self>) -> ByteOrder {
        self.byte_order_selects
            .get(ix)
            .and_then(|s| s.read(cx).selected_value().copied())
            .unwrap_or_default()
    }

    /// The byte order used by the edit dialog (first/active protocol).
    fn active_byte_order(&self, cx: &Context<Self>) -> ByteOrder {
        self.selected_byte_order(0, cx)
    }

    /// React to a value-type change in the edit dialog: update the editor's
    /// type/width, reload the bits, and re-fill the typed/hex input previews.
    fn on_value_type_changed(
        &mut self,
        vt: ValueType,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let byte_order = self.active_byte_order(cx);
        let (area, addr) = {
            let e = self.bit_editor.read(cx);
            (e.area, e.addr)
        };
        self.sync_string_len_from_input(window, cx);
        {
            let s = self.store.read().unwrap();
            self.bit_editor.update(cx, |e, _| {
                e.area = area;
                e.addr = addr;
                e.value_type = vt;
                e.load(&s, byte_order);
            });
        }
        self.sync_dialog_inputs(vt, window, cx);
        cx.notify();
    }

    /// Read the `String(N)` length input and push it into the editor, resizing
    /// the raw view accordingly.
    fn set_input_if_changed(
        &self,
        input: &Entity<InputState>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if input.read(cx).value().to_string() != text {
            let text = text.to_string();
            input.update(cx, |s, cx| s.set_value(text, window, cx));
        }
    }

    /// Read the `String(N)` length input and push it into the editor, resizing
    /// the raw bit view to the new register count and reloading from the store.
    fn sync_string_len_from_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let len_text = self.str_len_input.read(cx).value().to_string();
        let chars = len_text.trim().parse::<usize>().unwrap_or(7).clamp(1, 120);
        let is_string = self.bit_editor.read(cx).value_type == ValueType::String;
        let changed = self.bit_editor.update(cx, |e, _| {
            if is_string && e.string_chars != chars {
                e.string_chars = chars;
                true
            } else {
                false
            }
        });
        if is_string && (changed || chars != self.bit_editor.read(cx).string_chars) {
            let s = self.store.read().unwrap();
            let byte_order = self.bit_editor.read(cx).byte_order;
            self.bit_editor.update(cx, |e, _| e.load(&s, byte_order));
            // Keep the length input showing a normalised value.
            let len = self.bit_editor.read(cx).string_chars.to_string();
            {
                let inp = self.str_len_input.clone();
                self.set_input_if_changed(&inp, &len, window, cx);
            }
        }
    }

    /// Re-fill the numeric, string, hex and length inputs from the editor's
    /// current words.
    fn sync_dialog_inputs(&mut self, vt: ValueType, window: &mut Window, cx: &mut Context<Self>) {
        let (text, hex) = {
            let s = self.store.read().unwrap();
            let e = self.bit_editor.read(cx);
            (e.display_value(&s), e.hex_text())
        };
        if vt == ValueType::String {
            let inp = self.str_input.clone();
            self.set_input_if_changed(&inp, &text, window, cx);
        } else {
            let inp = self.num_input.clone();
            self.set_input_if_changed(&inp, &text, window, cx);
        }
        let hex_inp = self.hex_input.clone();
        self.set_input_if_changed(&hex_inp, &hex, window, cx);
    }

    fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dark = Theme::global(cx).is_dark();
        Theme::change(
            if dark {
                ThemeMode::Light
            } else {
                ThemeMode::Dark
            },
            Some(window),
            cx,
        );
    }

    fn reset_area(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.store.write().unwrap().reset_area(self.active_area);
        window.push_notification(
            Notification::info(format!("{} 已重置为 0", self.active_area.name_zh())),
            cx,
        );
        self.push_snapshot(cx);
    }

    fn random_fill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let area = self.active_area;
        {
            let mut s = self.store.write().unwrap();
            let len = s.len(area);
            let mut values = Vec::with_capacity(len);
            let mut seed = len as u64 ^ 0xDEAD_BEEF;
            for _ in 0..len {
                seed = seed
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let v = if area.is_bit() {
                    ((seed >> 33) & 1) as u16
                } else {
                    ((seed >> 17) ^ (seed >> 1)) as u16
                };
                values.push(v);
            }
            s.set_range(area, 0, &values, "UI");
        }
        window.push_notification(
            Notification::info(format!("{} 已填充随机值", area.name_zh())),
            cx,
        );
        self.push_snapshot(cx);
    }

    fn open_edit_dialog(&mut self, row_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let area = self.active_area;
        if !area.writable() {
            window.push_notification(
                Notification::warning(format!("{} 为只读区域", area.name_zh())),
                cx,
            );
            return;
        }
        let byte_order = self.active_byte_order(cx);
        let row_addr = row_ix;

        // Load a default view derived from the selected type and pre-fill the
        // typed / hex / length inputs.
        {
            let vt = self
                .type_select
                .read(cx)
                .selected_value()
                .copied()
                .unwrap_or_default();
            self.bit_editor.update(cx, |e, _| {
                e.area = area;
                e.addr = row_addr;
                e.value_type = vt;
                e.load(&self.store.read().unwrap(), byte_order);
            });
            // Echo the current `String(N)` length into its input.
            if vt == ValueType::String {
                let len = self.bit_editor.read(cx).string_chars.to_string();
                let inp = self.str_len_input.clone();
                self.set_input_if_changed(&inp, &len, window, cx);
            }
            self.sync_dialog_inputs(vt, window, cx);
        }

        let store = self.store.clone();
        let table = self.table.clone();
        let bit_editor = self.bit_editor.clone();
        let type_select = self.type_select.clone();
        let num_input = self.num_input.clone();
        let str_input = self.str_input.clone();
        let hex_input = self.hex_input.clone();
        let str_len_input = self.str_len_input.clone();
        let area_zh = area.name_zh();
        let order_code = byte_order.code().to_string();

        // Apply the typed value (number / string) and write the registers.
        let apply_typed = Rc::new({
            let store = store.clone();
            let table = table.clone();
            let bit_editor = bit_editor.clone();
            let num_input = num_input.clone();
            let str_input = str_input.clone();
            let hex_input = hex_input.clone();
            move |window: &mut Window, cx: &mut App| -> Result<Vec<u16>, String> {
                let text = {
                    let e = bit_editor.read(cx);
                    if e.value_type == ValueType::String {
                        str_input.read(cx).value().to_string()
                    } else {
                        num_input.read(cx).value().to_string()
                    }
                };
                let result = {
                    let mut s = store.write().unwrap();
                    bit_editor.update(cx, |e, _| e.apply_typed(&mut s, &text))
                };
                if let Ok(words) = &result {
                    let addr = bit_editor.read(cx).addr;
                    table.update(cx, |t, cx| {
                        for (k, w) in words.iter().enumerate() {
                            t.delegate_mut().poke_row(addr + k, *w, "UI");
                        }
                        cx.notify();
                    });
                    let hex = bit_editor.read(cx).hex_text();
                    hex_input.update(cx, |s, cx| s.set_value(hex, window, cx));
                }
                result
            }
        });

        // Apply raw hex register words and write them.
        let apply_hex = Rc::new({
            let store = store.clone();
            let table = table.clone();
            let bit_editor = bit_editor.clone();
            let hex_input = hex_input.clone();
            let num_input = num_input.clone();
            let str_input = str_input.clone();
            move |window: &mut Window, cx: &mut App| -> Result<Vec<u16>, String> {
                let text = hex_input.read(cx).value().to_string();
                let words = parse_hex_words(&text)?;
                let expected = bit_editor.read(cx).registers();
                if words.len() != expected {
                    return Err(format!(
                        "应输入 {expected} 个寄存器字，实际 {} 个",
                        words.len()
                    ));
                }
                let result = {
                    let mut s = store.write().unwrap();
                    bit_editor.update(cx, |e, _| {
                        e.set_words(&words);
                        e.apply_words(&mut s)
                    })
                };
                if let Ok(_) = &result {
                    let addr = bit_editor.read(cx).addr;
                    table.update(cx, |t, cx| {
                        for (k, w) in words.iter().enumerate() {
                            t.delegate_mut().poke_row(addr + k, *w, "UI");
                        }
                        cx.notify();
                    });
                    let text = bit_editor.read(cx).bits_text();
                    let is_string = bit_editor.read(cx).value_type == ValueType::String;
                    if is_string {
                        str_input.update(cx, |s, cx| s.set_value(text, window, cx));
                    } else {
                        num_input.update(cx, |s, cx| s.set_value(text, window, cx));
                    }
                }
                result
            }
        });

        // Apply the current in-memory bit pattern (used by OK in bit mode).
        let apply_words = Rc::new({
            let store = store.clone();
            let table = table.clone();
            let bit_editor = bit_editor.clone();
            move |_window: &mut Window, cx: &mut App| -> Result<Vec<u16>, String> {
                let result = {
                    let mut s = store.write().unwrap();
                    bit_editor.update(cx, |e, _| e.apply_words(&mut s))
                };
                if let Ok(words) = &result {
                    let addr = bit_editor.read(cx).addr;
                    table.update(cx, |t, cx| {
                        for (k, w) in words.iter().enumerate() {
                            t.delegate_mut().poke_row(addr + k, *w, "UI");
                        }
                        cx.notify();
                    });
                }
                result
            }
        });

        window.open_dialog(cx, move |dialog, _window, _cx| {
            let bits = bit_editor.read(_cx);
            let vt = bits.value_type;
            let is_string = vt == ValueType::String;
            let nbits = bits.bits.len();
            let regs = bits.registers();
            let byte_len = bits.byte_len();
            let view_mode = bits.view_mode;
            let hex = bits.hex_text();
            let order = order_code.clone();

            // Data type selector row.
            let type_row = h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(_cx.theme().muted_foreground)
                        .child("数据类型"),
                )
                .child(
                    Select::new(&type_select)
                        .small()
                        .menu_width(gpui::rems(17.)),
                );

            // `String(N)` length row (only for strings).
            let str_len_row = if is_string {
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(_cx.theme().muted_foreground)
                            .child("字符数 / 字节数 (String(N))"),
                    )
                    .child(div().flex_1())
                    .child(div().w(px(120.)).child(Input::new(&str_len_input).small()))
                    .into_any_element()
            } else {
                div().into_any_element()
            };

            // Bit / Hex display-mode toggle.
            let mode_row = h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(_cx.theme().muted_foreground)
                        .child("显示方式"),
                )
                .child(
                    Button::new("mode-bits")
                        .small()
                        .when(view_mode == ViewMode::Bits, |b| b.primary())
                        .when(view_mode != ViewMode::Bits, |b| b.ghost())
                        .label("位编辑")
                        .on_click({
                            let editor = bit_editor.clone();
                            move |_, _window, cx| {
                                editor.update(cx, |e, _| e.view_mode = ViewMode::Bits);
                                _window.refresh();
                            }
                        }),
                )
                .child(
                    Button::new("mode-hex")
                        .small()
                        .when(view_mode == ViewMode::Hex, |b| b.primary())
                        .when(view_mode != ViewMode::Hex, |b| b.ghost())
                        .label("16进制")
                        .on_click({
                            let editor = bit_editor.clone();
                            move |_, _window, cx| {
                                editor.update(cx, |e, _| e.view_mode = ViewMode::Hex);
                                _window.refresh();
                            }
                        }),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(_cx.theme().muted_foreground)
                        .child(format!("{regs} 寄存器 · {byte_len} 字节")),
                );

            // Typed input (number or string) + auto-fill button.
            let typed_row = h_flex()
                .gap_2()
                .items_center()
                .child(div().flex_1().child(if is_string {
                    Input::new(&str_input).flex_1().into_any_element()
                } else {
                    Input::new(&num_input).flex_1().into_any_element()
                }))
                .child(
                    Button::new("apply-value")
                        .small()
                        .primary()
                        .label("填充")
                        .on_click({
                            let apply_typed = apply_typed.clone();
                            move |_, window, cx| match apply_typed(window, cx) {
                                Ok(words) => {
                                    window.push_notification(
                                        Notification::success(format!(
                                            "已写入 {} 个寄存器",
                                            words.len()
                                        )),
                                        cx,
                                    );
                                }
                                Err(e) => {
                                    window.push_notification(Notification::warning(e), cx);
                                }
                            }
                        }),
                );

            // Value preview line.
            let preview_row = h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(_cx.theme().muted_foreground)
                        .child(format!("{} · 字节序 {}", vt.label(), order)),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(hex),
                );

            // Hex editor panel (shown in Hex mode).
            let hex_editor = v_flex()
                .gap_2()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().flex_1().child(Input::new(&hex_input).flex_1()))
                        .child(
                            Button::new("apply-hex")
                                .small()
                                .primary()
                                .label("应用16进制")
                                .on_click({
                                    let apply_hex = apply_hex.clone();
                                    move |_, window, cx| match apply_hex(window, cx) {
                                        Ok(w) => {
                                            window.push_notification(
                                                Notification::success(format!(
                                                    "已写入 {} 个寄存器",
                                                    w.len()
                                                )),
                                                cx,
                                            );
                                        }
                                        Err(e) => {
                                            window.push_notification(Notification::warning(e), cx);
                                        }
                                    }
                                }),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(_cx.theme().muted_foreground)
                        .child("按寄存器字输入 16 位十六进制，空格分隔，如 1234 5678"),
                );

            // Bit grid: 8 bits per row.
            let bit_rows = (0..nbits)
                .collect::<Vec<_>>()
                .chunks(8)
                .map(|chunk| {
                    h_flex()
                        .gap_2()
                        .justify_center()
                        .items_center()
                        .children(chunk.iter().map(|&i| {
                            let editor = bit_editor.clone();
                            let num_input = num_input.clone();
                            let str_input = str_input.clone();
                            let hex_input = hex_input.clone();
                            div()
                                .flex_row()
                                .content_center()
                                .w_16()
                                .child(
                                    Checkbox::new(("bit", i))
                                        .large()
                                        .checked(bits.bits[i])
                                        .on_click(move |&on, w, cx| {
                                            let (text, hex) = editor.update(cx, |e, _| {
                                                e.set_bit(i, on);
                                                (e.bits_text(), e.hex_text())
                                            });
                                            if is_string {
                                                str_input
                                                    .update(cx, |s, cx| s.set_value(text, w, cx));
                                            } else {
                                                num_input
                                                    .update(cx, |s, cx| s.set_value(text, w, cx));
                                            }
                                            hex_input.update(cx, |s, cx| s.set_value(hex, w, cx));
                                        }),
                                )
                                .text_color(_cx.theme().muted_foreground)
                                .child(i.to_string())
                                .into_any_element()
                        }))
                })
                .collect::<Vec<_>>();

            let grid_label = h_flex()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(_cx.theme().muted_foreground)
                        .child("按位编辑（每行 8 位，bit0 = 最低位）"),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(_cx.theme().muted_foreground)
                        .child(format!("{} 位", nbits)),
                );

            let editor_panel = if view_mode == ViewMode::Hex {
                hex_editor.into_any_element()
            } else {
                v_flex()
                    .gap_2()
                    .child(grid_label)
                    .child(
                        v_flex()
                            .border_1()
                            .border_color(_cx.theme().border)
                            .rounded(_cx.theme().radius)
                            .p_3()
                            .gap_2()
                            .children(bit_rows),
                    )
                    .into_any_element()
            };

            let footer = div()
                .text_size(px(12.))
                .text_color(_cx.theme().muted_foreground)
                .child(format!(
                    "占用 {regs} 个寄存器 (地址 {:#06X} ~ {:#06X}) · {byte_len} 字节 · {}",
                    row_addr,
                    row_addr + regs - 1,
                    vt.label_zh(),
                ));

            dialog
                .title(format!("编辑寄存器 · {area_zh} · 地址 {row_addr:#06X}"))
                .width(px(640.))
                .child(
                    v_flex()
                        .gap_3()
                        .px_2()
                        .child(type_row)
                        .child(str_len_row)
                        .child(mode_row)
                        .child(typed_row)
                        .child(preview_row)
                        .child(editor_panel)
                        .child(footer),
                )
                .on_ok({
                    let apply_hex = apply_hex.clone();
                    let apply_words = apply_words.clone();
                    move |_, window, cx| {
                        let result = if view_mode == ViewMode::Hex {
                            apply_hex(window, cx)
                        } else {
                            apply_words(window, cx)
                        };
                        match result {
                            Ok(_) => {
                                window.close_dialog(cx);
                                true
                            }
                            Err(e) => {
                                window.push_notification(Notification::warning(e), cx);
                                false
                            }
                        }
                    }
                })
        });
    }

    /// Parse the current parser hex input into `parser_state`.
    fn parse_hex_into_parser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hex = self.parser_hex_input.read(cx).value().to_string();
        match parse_modbus_hex(&hex) {
            Ok(info) => {
                self.parser_state.update(cx, |s, _| {
                    s.set_data(info.words.clone());
                    s.info = Some(info);
                    s.error = None;
                });
            }
            Err(e) => self.parser_state.update(cx, |s, _| s.error = Some(e)),
        }
        self.parser_sync_inputs(window, cx);
    }

    /// Keep the parser's `String(N)` length input in sync with its state.
    fn parser_sync_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let is_string = self.parser_state.read(cx).value_type == ValueType::String;
        if is_string {
            let len = self.parser_state.read(cx).string_chars.to_string();
            let inp = self.parser_str_len_input.clone();
            self.set_input_if_changed(&inp, &len, window, cx);
        }
    }

    /// Open the Modbus hex-parse / decode dialog.
    fn open_parser_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let parser_state = self.parser_state.clone();
        let parser_type_select = self.parser_type_select.clone();
        let parser_byte_order_select = self.parser_byte_order_select.clone();
        let parser_hex_input = self.parser_hex_input.clone();
        let parser_str_len_input = self.parser_str_len_input.clone();

        window.open_dialog(cx, move |dialog, _window, _cx| {
            let ps = parser_state.read(_cx);
            let vt = ps.value_type;
            let is_string = vt == ValueType::String;
            let nbits = ps.bits.len().min(64);
            let decoded = ps.decoded();
            let words_hex = ps.hex_text();
            let word_count = ps.words.len();
            let error = ps.error.clone();
            let info = ps.info.as_ref();

            // --- hex input row + parse button ---------------------------------
            let hex_row = h_flex()
                .gap_2()
                .items_center()
                .child(div().flex_1().child(Input::new(&parser_hex_input).flex_1()))
                .child(
                    Button::new("parse-now")
                        .small()
                        .primary()
                        .label("解析")
                        .on_click({
                            let ps = parser_state.clone();
                            let phx = parser_hex_input.clone();
                            move |_, window, cx| {
                                let hex = phx.read(cx).value().to_string();
                                match parse_modbus_hex(&hex) {
                                    Ok(info) => ps.update(cx, |s, _| {
                                        s.set_data(info.words.clone());
                                        s.info = Some(info);
                                        s.error = None;
                                    }),
                                    Err(e) => ps.update(cx, |s, _| s.error = Some(e)),
                                }
                                window.refresh();
                            }
                        }),
                );

            // --- header: MBAP fields + function code -------------------------
            let header = v_flex()
                .gap_1()
                .p_2()
                .rounded(_cx.theme().radius)
                .border_1()
                .border_color(_cx.theme().border)
                .child(
                    h_flex()
                        .gap_4()
                        .items_center()
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(_cx.theme().muted_foreground)
                                .child(format!(
                                    "事务 {} · 协议 {} · 长度 {} · 单元 {}",
                                    info.map(|i| format!("{:#06X}", i.tx_id)).unwrap_or_default(),
                                    info.map(|i| format!("{:#06X}", i.proto_id)).unwrap_or_default(),
                                    info.map(|i| i.length.to_string()).unwrap_or_default(),
                                    info.map(|i| i.unit_id.to_string()).unwrap_or_else(|| "—".into()),
                                )),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(
                                    info.map(|i| format!("功能码 {:#04X}", i.function_code))
                                        .unwrap_or_else(|| "功能码 —".to_string()),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(_cx.theme().primary)
                                .child(info.map(|i| i.name).unwrap_or_default()),
                        ),
                )
                .when(error.is_none(), |this| {
                    this.child(
                        div()
                            .text_size(px(12.))
                            .text_color(_cx.theme().muted_foreground)
                            .child(info.map(|i| i.note.clone()).unwrap_or_default()),
                    )
                })
                .when(error.is_some(), |this| {
                    this.child(
                        div()
                            .text_size(px(12.))
                            .text_color(_cx.theme().red)
                            .child(error.clone().unwrap_or_default()),
                    )
                });

            // --- value type / byte order / string length controls ------------
            let controls = h_flex()
                .gap_3()
                .items_center()
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(_cx.theme().muted_foreground)
                                .child("类型"),
                        )
                        .child(Select::new(&parser_type_select).small().menu_width(gpui::rems(11.))),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(_cx.theme().muted_foreground)
                                .child("字节序"),
                        )
                        .child(
                            Select::new(&parser_byte_order_select)
                                .small()
                                .menu_width(gpui::rems(18.)),
                        ),
                )
                .child(div().flex_1())
                .when(is_string, |this| {
                    this.child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(_cx.theme().muted_foreground)
                                    .child("String(N)"),
                            )
                            .child(div().w(px(64.)).child(Input::new(&parser_str_len_input).small())),
                    )
                });

            // --- decoded value + hex words -----------------------------------
            let value_line = h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(_cx.theme().muted_foreground)
                        .child(format!("{} 寄存器 · 值 =", word_count)),
                )
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(_cx.theme().green)
                        .child(decoded),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(_cx.theme().muted_foreground)
                        .child(words_hex),
                );

            // --- bit-wise grid (max 8 per row) -------------------------------
            let bit_rows = (0..nbits)
                .collect::<Vec<_>>()
                .chunks(8)
                .map(|chunk| {
                    h_flex()
                        .gap_2()
                        .justify_center()
                        .items_center()
                        .children(chunk.iter().map(|&i| {
                            let ps = parser_state.clone();
                            div()
                                .flex_row()
                                .content_center()
                                .w_16()
                                .child(
                                    Checkbox::new(gpui::SharedString::from(format!("parbit-{i}")))
                                        .small()
                                        .checked(ps.read(_cx).bits[i])
                                        .on_click(move |&on, window, cx| {
                                            ps.update(cx, |s, _| s.toggle_bit(i, on));
                                            window.refresh();
                                        }),
                                )
                                .text_color(_cx.theme().muted_foreground)
                                .child(i.to_string())
                        }))
                })
                .collect::<Vec<_>>();

            let grid_label = h_flex()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(_cx.theme().muted_foreground)
                        .child("按位查看（每行 8 位，bit0 = 最低位）"),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(_cx.theme().muted_foreground)
                        .child(format!("{} 位", nbits)),
                );

            dialog
                .title("Modbus 协议解析 (16 进制)".to_string())
                .width(px(680.))
                .child(
                    v_flex()
                        .gap_3()
                        .px_2()
                        .child(hex_row)
                        .child(header)
                        .child(controls)
                        .child(value_line)
                        .child(grid_label)
                        .child(
                            v_flex()
                                .gap_2()
                                .border_1()
                                .border_color(_cx.theme().border)
                                .rounded(_cx.theme().radius)
                                .p_3()
                                .children(bit_rows),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(_cx.theme().muted_foreground)
                                .child("在功能码方框粘贴完整 Modbus TCP 帧（MBAP + PDU）十六进制，可解析功能码并选择类型 / 字节序查看值及按位显示"),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    window.close_dialog(cx);
                    true
                })
        });
    }

    // -----------------------------------------------------------------------
    // Rendering
    // -----------------------------------------------------------------------

    fn render_title_bar(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let dark = Theme::global(cx).is_dark();

        // The draggable region: title block + spacer. Marked as a native
        // window-control drag area so Windows handles the move on `titlebar:
        // None` client-decorated windows; the buttons live outside it so they
        // stay clickable.
        let drag_area = h_flex()
            .id("title-bar-drag")
            .flex_1()
            .min_w_0()
            .gap_2()
            .items_center()
            .window_control_area(WindowControlArea::Drag)
            .child(Icon::new(IconName::LayoutDashboard).text_color(theme.primary))
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("IoT 协议模拟器"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .flex_1()
                    .child("Modbus TCP 仿真 · 可扩展 S7 / OPC-UA"),
            );

        h_flex()
            .h(px(44.))
            .px_3()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(drag_area)
            .child(
                Button::new("theme-toggle")
                    .ghost()
                    .icon(if dark { IconName::Sun } else { IconName::Moon })
                    .tooltip("切换浅色 / 深色主题")
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_theme(window, cx))),
            )
            .child(
                Button::new("close")
                    .ghost()
                    .icon(IconName::Close)
                    .tooltip("关闭窗口")
                    .on_click(|_, window, _| {
                        window.remove_window();
                    }),
            )
    }

    fn render_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .flex_1()
            .size_full()
            .overflow_hidden()
            .child(self.render_sidebar(window, cx))
            .child(self.render_content(window, cx))
    }

    fn render_sidebar(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let cxr: &Context<Self> = cx;
        let cards = self
            .protocols
            .iter()
            .enumerate()
            .map(|(i, card)| self.render_protocol_card(i, card, cxr).into_any_element())
            .collect::<Vec<_>>();
        v_flex()
            .w(px(320.))
            .h_full()
            .flex_shrink_0()
            .overflow_y_scrollbar()
            .p_3()
            .gap_3()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("协议服务器"),
            )
            .children(cards)
            .child(self.render_settings_card(cxr))
    }

    fn render_protocol_card(
        &self,
        ix: usize,
        card: &ProtocolCard,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let p = card.protocol.as_ref();
        let running = p.is_running();
        let state = p.state();
        let port_input = self.port_inputs[ix].clone();
        let byte_order_select = self.byte_order_selects[ix].clone();
        let state_badge = self.state_badge(&state, &theme);
        let stats_line = self.stats_line(&theme);

        v_flex()
            .gap_2()
            .p_3()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(p.name()),
                            )
                            .child(state_badge),
                    )
                    .child(
                        Button::new(("toggle", ix))
                            .label(if running { "停止" } else { "启动" })
                            .when(running, |b| b.danger().outline())
                            .when(!running, |b| b.primary())
                            .small()
                            .on_click(cx.listener(
                                move |this, _: &gpui::ClickEvent, window, cx| {
                                    this.toggle_protocol(ix, window, cx);
                                },
                            )),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child(p.description()),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child("监听端口"),
                    )
                    .child(div().flex_1())
                    .child(Input::new(&port_input).small().disabled(running)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child("字节序"),
                    )
                    .child(div().flex_1())
                    .child(
                        Select::new(&byte_order_select)
                            .small()
                            .disabled(running)
                            .menu_width(gpui::rems(19.)),
                    ),
            )
            .child(stats_line)
            .when(matches!(state, ServerState::Error(_)), |this| {
                let msg = match &state {
                    ServerState::Error(m) => m.clone(),
                    _ => String::new(),
                };
                this.child(div().text_size(px(12.)).text_color(theme.red).child(msg))
            })
    }

    fn state_badge(&self, state: &ServerState, theme: &gpui_component::Theme) -> impl IntoElement {
        let (color, pulse) = match state {
            ServerState::Running => (theme.green, true),
            ServerState::Starting | ServerState::Stopping => (theme.yellow, true),
            ServerState::Stopped => (theme.muted_foreground, false),
            ServerState::Error(_) => (theme.red, false),
        };
        h_flex()
            .gap_1()
            .items_center()
            .child(
                div()
                    .w_2()
                    .h_2()
                    .rounded_full()
                    .bg(color)
                    .when(pulse, |this| this.cursor_pointer()),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(color)
                    .child(state.label()),
            )
    }

    fn stats_line(&self, theme: &gpui_component::Theme) -> impl IntoElement {
        let s = &self.stats;
        let clients = s.current_clients.load(Ordering::Relaxed);
        let peak = s.peak_clients.load(Ordering::Relaxed);
        let requests = s.total_requests.load(Ordering::Relaxed);
        let written = s.cells_written.load(Ordering::Relaxed);
        let errors = s.error_responses.load(Ordering::Relaxed);

        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child("当前连接"),
                    )
                    .child(div().text_size(px(12.)).child(clients.to_string())),
            )
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child("最大连接"),
                    )
                    .child(div().text_size(px(12.)).child(peak.to_string())),
            )
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child("总请求"),
                    )
                    .child(div().text_size(px(12.)).child(requests.to_string())),
            )
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child("写入单元"),
                    )
                    .child(div().text_size(px(12.)).child(written.to_string())),
            )
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child("错误响应"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(if errors > 0 {
                                theme.red
                            } else {
                                theme.muted_foreground
                            })
                            .child(errors.to_string()),
                    ),
            )
    }

    fn render_settings_card(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let on = self.auto_sim.load(Ordering::Relaxed);
        v_flex()
            .gap_2()
            .p_3()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("模拟设置"),
            )
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(div().text_size(px(14.)).child("自动模拟数据"))
                    .child(Switch::new("auto-sim").checked(on).on_click(cx.listener(
                        |this, &checked, _, cx| {
                            this.auto_sim.store(checked, Ordering::Relaxed);
                            cx.notify();
                        },
                    ))),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child("每 200ms 随机更新若干寄存器，便于观察实时效果"),
            )
    }

    fn render_content(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let area = self.active_area;
        let len = self.store.read().unwrap().len(area);
        v_flex()
            .flex_1()
            .h_full()
            .gap_2()
            .p_3()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        TabBar::new("area-tabs")
                            .selected_index(self.active_area.index())
                            .on_click(cx.listener(|this, &ix: &usize, window, cx| {
                                let area = ALL_AREAS[ix.min(ALL_AREAS.len() - 1)];
                                this.set_active_area(area, window, cx);
                            }))
                            .child(Tab::new().label(format!(
                                "线圈 ({})",
                                self.store.read().unwrap().len(Area::Coils)
                            )))
                            .child(Tab::new().label(format!(
                                "离散输入 ({})",
                                self.store.read().unwrap().len(Area::DiscreteInputs)
                            )))
                            .child(Tab::new().label(format!(
                                "保持寄存器 ({})",
                                self.store.read().unwrap().len(Area::HoldingRegisters)
                            )))
                            .child(Tab::new().label(format!(
                                "输入寄存器 ({})",
                                self.store.read().unwrap().len(Area::InputRegisters)
                            ))),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .child(if area.writable() {
                                        "双击行编辑数值 · "
                                    } else {
                                        "只读区域 · "
                                    })
                                    .child(format!("共 {} 个地址", len)),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("reset-area")
                                    .ghost()
                                    .small()
                                    .label("重置")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.reset_area(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("rand-area")
                                    .ghost()
                                    .small()
                                    .label("随机填充")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.random_fill(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("modbus-parse")
                                    .ghost()
                                    .small()
                                    .label("Modbus 解析")
                                    .tooltip("解析 Modbus TCP 帧（16 进制），查看功能码 / 类型 / 字节序 / 按位显示")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_parser_dialog(window, cx);
                                    })),
                            ),
                    ),
            )
            .child(
                Table::new(&self.table)
                    .stripe(true)
                    .bordered(true)
                    .scrollbar_visible(true, true)
                    .with_size(px(44.)),
            )
    }

    fn render_status_bar(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let revision = self.last_revision;
        let stats_text = self
            .protocols
            .iter()
            .map(|card| {
                let p = card.protocol.as_ref();
                format!(
                    "{} [{}]: {} (端口 {}) · 连接 {}",
                    p.name(),
                    p.id(),
                    p.state().label(),
                    p.port(),
                    self.stats.current_clients.load(Ordering::Relaxed),
                )
            })
            .collect::<Vec<_>>()
            .join("  |  ");

        h_flex()
            .h(px(30.))
            .px_3()
            .gap_4()
            .items_center()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child(stats_text),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child(format!("数据版本 {}", revision)),
            )
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .size_full()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.render_title_bar(window, cx))
            .child(self.render_body(window, cx))
            .child(self.render_status_bar(window, cx))
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
    }
}
