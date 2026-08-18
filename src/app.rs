//! Main application view: title bar, protocol sidebar, register table, dialogs.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use gpui::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, AsyncApp, Context, Entity,
    IntoElement, ParentElement as _, Render, Styled as _, Subscription, Task, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputState},
    label,
    notification::Notification,
    scroll::ScrollableElement as _,
    select::{Select, SelectItem, SelectState},
    switch::Switch,
    tab::{Tab, TabBar},
    table::{Column, Table, TableDelegate, TableEvent, TableState},
    v_flex, ActiveTheme as _, Icon, IconName, IndexPath, Root, Sizable as _, Theme, ThemeMode,
    WindowExt as _,
};

use iot_mock::model::{
    shared_store, snapshot_area, Area, AreaSnapshot, ByteOrder, DataWidth, RegisterStore, Row,
    SharedStore, ValueType, ALL_AREAS, DEFAULT_AREA_SIZE,
};
use iot_mock::protocol::{
    modbus::{ModbusTcpServer, DEFAULT_PORT},
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

/// The register table delegate: columns + per-area row rendering.
pub struct RegTableDelegate {
    area: Area,
    rows: Vec<Row>,
    prev: Vec<u16>,
    columns: Vec<Column>,
}

impl RegTableDelegate {
    fn new() -> Self {
        Self {
            area: Area::HoldingRegisters,
            rows: Vec::new(),
            prev: Vec::new(),
            columns: vec![
                Column::new("addr", "地址").width(90.).resizable(false),
                Column::new("name", "名称").width(170.).resizable(false),
                Column::new("value", "值").width(170.).resizable(false),
                Column::new("writer", "最后写入")
                    .width(140.)
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

/// Editor state for the register bit/typed edit dialog.
///
/// Kept as its own mutable entity because the dialog's render function is
/// re-invoked every frame; the checkboxes and inputs read/write this entity so
/// their state survives re-renders. `bits[0]` is the LSB.
pub struct BitEditorState {
    /// 32 bools; only `width.bits()` of them are meaningful.
    bits: [bool; 32],
    width: DataWidth,
    addr: usize,
    area: Area,
    byte_order: ByteOrder,
    /// The type used to interpret and "auto-fill" the registers.
    value_type: ValueType,
    /// Last accepted string for `ValueType::String` (may be many registers).
    pending_string: String,
}

impl BitEditorState {
    fn new() -> Self {
        Self {
            bits: [false; 32],
            width: DataWidth::Bits16,
            addr: 0,
            area: Area::HoldingRegisters,
            byte_order: ByteOrder::Abcd,
            value_type: ValueType::Uint16,
            pending_string: String::new(),
        }
    }

    /// Load the current value at `addr` into the bit array for the given width.
    /// 32-bit values span `addr` and `addr + 1`, decoded by `byte_order`.
    fn load(
        &mut self,
        store: &RegisterStore,
        area: Area,
        addr: usize,
        width: DataWidth,
        byte_order: ByteOrder,
    ) -> bool {
        let value32 = match width {
            DataWidth::Bits16 => store.get(area, addr).unwrap_or(0) as u32,
            DataWidth::Bits32 => {
                let w0 = store.get(area, addr).unwrap_or(0);
                let w1 = store.get(area, addr + 1).unwrap_or(0);
                byte_order.decode_u32([w0, w1])
            }
        };
        self.area = area;
        self.addr = addr;
        self.width = width;
        self.byte_order = byte_order;
        self.bits = [false; 32];
        for i in 0..width.bits() {
            self.bits[i] = (value32 >> i) & 1 == 1;
        }
        true
    }

    /// Overwrite the raw bit view from register words (16-bit -> 16 bits,
    /// 32-bit -> 32 bits via byte order).
    fn set_words(&mut self, words: &[u16]) {
        let word0 = words.first().copied().unwrap_or(0);
        let value32 = match self.width {
            DataWidth::Bits16 => word0 as u32,
            DataWidth::Bits32 => {
                let w1 = words.get(1).copied().unwrap_or(0);
                self.byte_order.decode_u32([word0, w1])
            }
        };
        self.bits = [false; 32];
        for i in 0..self.width.bits() {
            self.bits[i] = (value32 >> i) & 1 == 1;
        }
    }

    fn set_bit(&mut self, ix: usize, on: bool) {
        if ix < self.width.bits() {
            self.bits[ix] = on;
        }
    }

    /// Assemble the bits into a 32-bit value (bit 0 = LSB).
    fn value_u32(&self) -> u32 {
        let mut v = 0u32;
        for i in 0..self.width.bits() {
            if self.bits[i] {
                v |= 1 << i;
            }
        }
        v
    }

    fn value_u16(&self) -> u16 {
        self.value_u32() as u16
    }

    /// Apply a typed value (number or string) from `text` into the store at
    /// `addr`, then re-sync the raw bit view. On success returns the words that
    /// were written; on parse/range failure returns `Err(message)`.
    fn apply_typed(&mut self, store: &mut RegisterStore, text: &str) -> Result<Vec<u16>, String> {
        if !self.area.writable() {
            return Err("区域只读".to_string());
        }
        let budget = store.len(self.area).saturating_sub(self.addr);
        let words = self.value_type.encode_text(text, self.byte_order, budget)?;
        if self.value_type == ValueType::String {
            self.pending_string = text.to_string();
            let n = words.len().min(2);
            self.set_words(&words[..n]);
            // For strings wider than 2 registers, widen the raw view so bit
            // editing (if any) still covers the first two registers.
            if words.len() > 2 {
                self.width = DataWidth::Bits32;
            }
        } else {
            self.set_words(&words);
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
            .range(self.area, self.addr, self.width.registers())
            .unwrap_or_default();
        self.value_type.decode_words(&words, self.byte_order)
    }

    /// Render the current in-memory bit pattern as a typed string (kept in sync
    /// with the typed input each time a checkbox is toggled).
    fn bits_text(&self) -> String {
        let words = match self.width {
            DataWidth::Bits16 => vec![self.value_u16()],
            DataWidth::Bits32 => self.byte_order.encode_u32(self.value_u32()).to_vec(),
        };
        self.value_type.decode_words(&words, self.byte_order)
    }

    /// Number of registers to read/write for the current type/dialog view.
    fn registers(&self) -> usize {
        self.value_type
            .fixed_registers()
            .unwrap_or(self.width.registers())
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
            TableState::new(RegTableDelegate::new(), window, cx)
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
    /// type/width, reload the bits, and re-fill the typed input preview.
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
        let width = match vt.fixed_registers() {
            Some(1) => DataWidth::Bits16,
            Some(_) => DataWidth::Bits32,
            None => DataWidth::Bits16, // string starts as short view
        };
        {
            let s = self.store.read().unwrap();
            self.bit_editor.update(cx, |e, _| {
                e.load(&s, area, addr, width, byte_order);
                e.value_type = vt;
            });
        }
        // Re-fill the typed input with the decoded value.
        let text = self
            .bit_editor
            .read(cx)
            .display_value(&self.store.read().unwrap());
        self.fill_typed_input(vt, &text, window, cx);
        cx.notify();
    }

    /// Fill the numeric or string input with `text` depending on the type.
    fn fill_typed_input(
        &mut self,
        vt: ValueType,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = text.to_string();
        if vt == ValueType::String {
            self.str_input
                .update(cx, |s, cx| s.set_value(text, window, cx));
        } else {
            self.num_input
                .update(cx, |s, cx| s.set_value(text, window, cx));
        }
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

        // Load a default view derived from the selected type and pre-fill inputs.
        {
            let vt = self
                .type_select
                .read(cx)
                .selected_value()
                .copied()
                .unwrap_or_default();
            let width = match vt.fixed_registers() {
                Some(1) | None => DataWidth::Bits16,
                Some(_) => DataWidth::Bits32,
            };
            let text = {
                let s = self.store.read().unwrap();
                self.bit_editor.update(cx, |e, _| {
                    e.load(&s, area, row_addr, width, byte_order);
                    e.value_type = vt;
                });
                self.bit_editor.read(cx).display_value(&s)
            }; // `s` dropped here
            self.fill_typed_input(vt, &text, window, cx);
        }

        let store = self.store.clone();
        let table = self.table.clone();
        let bit_editor = self.bit_editor.clone();
        let type_select = self.type_select.clone();
        let num_input = self.num_input.clone();
        let str_input = self.str_input.clone();
        let area_zh = area.name_zh();
        let order_code = byte_order.code().to_string();

        // Shared "apply typed value → fill registers" routine used by the Apply
        // button, checkbox toggles (indirectly) and the OK action.
        let apply = {
            let store = store.clone();
            let table = table.clone();
            let bit_editor = bit_editor.clone();
            let num_input = num_input.clone();
            let str_input = str_input.clone();
            move |_window: &mut Window, cx: &mut App| -> Result<Vec<u16>, String> {
                let (ty, text) = {
                    let e = bit_editor.read(cx);
                    (
                        e.value_type,
                        if e.value_type == ValueType::String {
                            str_input.read(cx).value().to_string()
                        } else {
                            num_input.read(cx).value().to_string()
                        },
                    )
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
                }
                let _ = ty;
                result
            }
        };

        window.open_dialog(cx, move |dialog, _window, _cx| {
            let bits = bit_editor.read(_cx);
            let width = bits.width;
            let nbits = width.bits();
            let vt = bits.value_type;
            let is_string = vt == ValueType::String;
            let current = bits.value_u32();
            let hex = match width {
                DataWidth::Bits16 => format!("0x{:04X}", current as u16),
                DataWidth::Bits32 => format!("0x{:08X}", current),
            };

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
                            let apply = apply.clone();
                            move |_, window, cx| match apply(window, cx) {
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
            let value_line = h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(_cx.theme().muted_foreground)
                        .child(format!("{} · 字节序 {}", vt.label(), order_code)),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(hex),
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
                            div()
                                .flex_row()
                                .content_center()
                                .w_16()
                                .child(
                                    Checkbox::new(("bit", i))
                                        .large()
                                        .checked(bits.bits[i])
                                        .on_click(move |&on, w, cx| {
                                            let text = editor.update(cx, |e, _| {
                                                e.set_bit(i, on);
                                                e.bits_text()
                                            });
                                            if is_string {
                                                str_input
                                                    .update(cx, |s, cx| s.set_value(text, w, cx));
                                            } else {
                                                num_input
                                                    .update(cx, |s, cx| s.set_value(text, w, cx));
                                            }
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

            dialog
                .title(format!("编辑寄存器 · {area_zh} · 地址 {row_addr:#06X}"))
                .width(px(640.))
                .child(
                    v_flex()
                        .gap_3()
                        .px_2()
                        .child(type_row)
                        .child(typed_row)
                        .child(value_line)
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
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(_cx.theme().muted_foreground)
                                .child(format!(
                                    "占用 {} 个寄存器 (地址 {:#06X} ~ {:#06X}) · {}",
                                    bits.registers(),
                                    row_addr,
                                    row_addr + bits.registers() - 1,
                                    vt.label_zh(),
                                )),
                        ),
                )
                .on_ok({
                    let apply = apply.clone();
                    move |_, window, cx| match apply(window, cx) {
                        Ok(_) => {
                            window.close_dialog(cx);
                            true
                        }
                        Err(e) => {
                            window.push_notification(Notification::warning(e), cx);
                            false
                        }
                    }
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
        h_flex()
            .h(px(44.))
            .px_3()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
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
                            .child("Modbus TCP 仿真 · 可扩展 S7 / OPC-UA"),
                    ),
            )
            .child(div().flex_1())
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
