# IoT 协议模拟器 — 开发任务记录

> 本文件用于记录本项目的开发任务、当前进度与后续计划，方便随时回顾。
> 最后更新：随每次变更维护。

## 项目概述

基于 **Rust + GPUI + gpui-component** 的桌面端 IoT 协议模拟器。
当前支持 **Modbus TCP** 服务端模拟，采用可扩展架构（统一 `Protocol` trait），
后续可扩展 S7 / OPC-UA / MQTT 等协议。

运行方式：`cargo run --release`
目录结构、架构与扩展指南详见 [README.md](../README.md)。

---

## 任务清单

### ✅ 已完成：阶段一 — Modbus TCP 基础 + 实时数据

| # | 任务 | 说明 | 状态 |
|---|------|------|------|
| 1 | 项目骨架 | `Cargo.toml`（gpui 0.2.2 / gpui-component 0.5.1）、bin+lib 分离 | ✅ |
| 2 | 共享数据模型 | `model.rs`：`RegisterStore`、`Area`、`Row`、快照、自动模拟 | ✅ |
| 3 | 协议抽象层 | `protocol/mod.rs`：`Protocol` trait、`ServerStats`、`ProtocolCard`、`ServerState` | ✅ |
| 4 | Modbus TCP 服务端 | `protocol/modbus.rs`：MBAP+PDU，FC 01/02/03/04/05/06/0F/10，异常码 | ✅ |
| 5 | GPUI 界面 | `app.rs`：标题栏 / 协议侧栏 / 数据表 / 编辑弹窗 / 状态栏 | ✅ |
| 6 | 实时显示 | 200ms 刷新，改动行高亮，四数据区切换，协议统计 | ✅ |
| 7 | 单元测试 | PDU 解析、位打包、越界异常、读写往返、统计峰值 | ✅ |
| 8 | 端到端集成测试 | `tests/modbus_tcp.rs`：真实 TCP 客户端 ↔ 服务器 ↔ 共享存储 | ✅ |
| 9 | 工程洁净 | `cargo build` / `cargo clippy` 无警告，8 用例全绿 | ✅ |
| 10 | GUI 启动验证 | 窗口正常创建，进程稳定运行 | ✅ |

### ✅ 已完成：阶段二 — Bit 位编辑 + 32 位支持 + 4 种字节序

用户新增需求（本次会话）：

> 1. 修改数据可使用 **Bit 位修改**：如 16 位显示 16 个 CheckBox，勾选对应位。
> 2. 支持 **32 位数据修改**（占 2 个寄存器地址），同样支持。
> 3. 支持 **Modbus TCP 对应的 4 种字节序**，在**启动 Server 时选择**。

| # | 任务 | 说明 | 状态 |
|---|------|------|------|
| 1 | 字节序/宽度枚举 | `model.rs` 新增 `ByteOrder`（ABCD/CDAB/BADC/DCBA）与 `DataWidth`（16/32），含 `encode_u32`/`decode_u32` | ✅ |
| 2 | 字节序单元测试 | `model.rs::tests`：4 种字节序编码/解码/往返/连线模式 | ✅ 全绿 |
| 3 | 协议卡加字节序下拉 | `Select` 选字节序；`ByteOrderItem` 本地包装规避孤儿规则；运行时禁用，启动时读取生效 | ✅ |
| 4 | Bit 编辑器状态 | `BitEditorState`：保存 32 bit、宽度、地址、字节序；`load`/`set_bit`/`write`/`value_words` | ✅ |
| 5 | 编辑弹窗改造 | 双击打开 Bit 编辑弹窗；16/32 位宽度切换；16 位=16 CheckBox、32 位=32 CheckBox；实时 hex 预览与占用寄存器提示 | ✅ |
| 6 | 双击关联字节序 | 用协议当前选择字节序解码/编码 32 位；确认后写回并即时刷新表格 | ✅ |
| 7 | 构建/测试/验证 | 编译、clippy 无警告；14 单测+1 集成全绿；GUI 启动并响应正常 | ✅ |

**踩坑记录：**
- `SelectItem` 不能直接为外部类型 `ByteOrder` 实现（孤儿规则）→ 用本地 `ByteOrderItem` newtype。
- `Button::new(("width", 16))` 字面量推断为 `i32`，需显式 `16usize`。
- `start_protocol` 需先不可变借用读端口/字节序，再 `self.protocols.get_mut`（避免 E0502）。
- title bar 遗留的关闭按钮补全为 `window.remove_window()`。

### ✅ 已完成：阶段三 — Bit 8 位一行 + 基础类型自动填充

用户新增需求（本次会话）：

> 1. 按位编辑：**8 位一行**显示（每行 8 个 CheckBox）。
> 2. 支持输入 **Int32（及 Uint16/Int16/Uint32）、Float32、字符串 / 字符（输入 A → UTF-8 转字符）** 等基础类型，**自动填充到寄存器**。

| # | 任务 | 说明 | 状态 |
|---|------|------|------|
| 1 | ValueType 枚举 | `model.rs` 新增 `ValueType`（UInt16/Int16/UInt32/Int32/Float32/String）+ `encode_text`/`decode_words` | ✅ |
| 2 | 字符串转换 | `store_string`/`load_string`：UTF-8 每寄存器 2 字节填充、"A"→字符、NUL 补齐与截断 | ✅ |
| 3 | 类型单元测试 | UInt16/Int32(全字节序)/Float32/字符串(含多字节字符)/越界拒绝/非法数值 | ✅ 全绿 |
| 4 | 类型下拉 | 编辑弹窗加 `Select` 选基础类型；`ValueTypeItem` newtype；变化时订阅回调重载 | ✅ |
| 5 | Bit 8 位一行 | 位网格改为每行 8 个 CheckBox（16 位=2 行、32 位=4 行），勾选联动输入框 | ✅ |
| 6 | 自动填充 | 输入数值/Float/字符串 → 「应用 → 填充寄存器」按钮或回车 → 解析并按字节序写入；字符串多寄存器 | ✅ |
| 7 | 构建/测试/验证 | 编译、clippy 无警告；21 单测+1 集成全绿；GUI 启动并响应正常 | ✅ |

**踩坑记录：**
- 编辑弹窗的 `Fn` 渲染闭包只有 `&mut App`，不能 `cx.listener` 调 AppView 方法 → 用实体 clone + 内联闭包（`apply` 复用给「应用」按钮与 OK）。
- 打开弹窗时读到 store 的 `RwLockReadGuard` 需先 drop 再 `fill_typed_input`（避免 E0502）。
- clippy `approx_constant`：float 测试用 `3.14` 会被当成 π 近似值拒绝 → 改用 `2.5`。
- 对话框「OK」直接走 `apply_typed`（写回 + 同步 bits + 刷新表格），保证与「应用」一致。

### ✅ 已完成：阶段四 — 16 进制编辑切换 + String(N) + 标题栏拖动

用户新增需求（本次会话）：

> 1. 编辑数据界面：除了现在能按 Bit 位展示，也可以**切换 16 进制的展示编辑**。
> 2. 对 String，需要支持 **String(7) 这样的可设定字符个数**；同时 Bit 位和 16 进制也要**根据计算出来的 Byte 长度展示**。
> 3. 支持**现在自定义的标题栏拖动**。

| # | 任务 | 说明 | 状态 |
|---|------|------|------|
| 1 | 模型：字节↔寄存器换算 | `model.rs` 新增 `bytes_to_regs`（2 字节/寄存器，向上取整）、`encode_string_fixed`（按 N 字节定宽填充、超长报错） | ✅ |
| 2 | String 模型单元测试 | `bytes_to_regs` 取整、定宽填充、超长拒绝（字符与寄存器上限）、4 字节序往返 | ✅ 全绿 |
| 3 | 编辑器重构 | `BitEditorState` 移除固定 `width`，改 `Vec<bool> bits + reg_count`（任意寄存器宽度）；新增 `ViewMode`（Bits/Hex）、`string_chars`；`set_words/words/hex_text/byte_len/apply_words` | ✅ |
| 4 | String(N) 长度输入 | 类型选 String 时显示「字符数/字节数 (String(N))」输入；`str_len_input` 订阅 `InputEvent::Change` → 重算寄存器数并重载；防重入用 `set_input_if_changed` 守卫 | ✅ |
| 5 | 显示方式切换 | 弹窗加「位编辑 / 16进制」按钮（`.when` 切换 primary/ghost 高亮），`_window.refresh()` 触发重绘 | ✅ |
| 6 | 16 进制编辑 | Hex 模式显示空格分隔的 16 位寄存器字输入；`parse_hex_words` 解析（容错 `0x`、校验字数）；「应用16进制」按原始字写入 | ✅ |
| 7 | 位/Hex/类型联动 | 位勾选同步 typed 与 hex 预览；Hex 应用同步 typed；类型切换重载；OK 按当前模式提交（Hex=apply_hex，Bits=apply_words） | ✅ |
| 8 | 字节/寄存器展示 | 位视图显示全部占用位、模式行与底部标注「占用 X 寄存器 · Y 字节」（如 String(7)=4 寄存器 · 7 字节） | ✅ |
| 9 | 标题栏拖动 | `title_bar_drag` 用 `window_control_area(WindowControlArea::Drag)`（原生 HTCAPTION），标题+空白区可拖，主题/关闭按钮在拖拽区外仍可点击 | ✅ |
| 10 | 构建/测试/验证 | `cargo build` 无警告；26 测试全绿（25 单测 + 1 集成） | ✅ |
| 11 | 文档 | `README.md` 更新编辑/字符串/标题栏功能说明 | ✅ |

**踩坑记录：**
- `Entity::update` 的闭包会捕获入参引用 → 传 `text: &str` 触发 E0521，需先 `to_string()` 转拥有型。
- `Vec<bool>` 的 `resize` 内 `b[..n].copy_from_slice(&self.bits[..n])` 触发 E0502 双重借用 → 先算局部 `n`。
- 对话框渲染闭包里 `cx` 是 `&mut App`，`cx.notify()` 需 `EntityId` → 改用 `_window.refresh()` 触发重绘。
- `on_ok` 闭包需 `'static`，不能捕获借用的 `bits` 守卫 → 只捕获 Copy 的 `view_mode` 与 `Rc` 共享的 apply 闭包。
- 多个按钮/OK 复用同一定制闭包 → 用 `Rc::new(closure)` + clone（定制闭包不可 Clone）。
- `apply_hex` 校验输入字数与当前寄存器数一致，避免写入不同宽度。
- `sync_string_len_from_input` 与 `sync_dialog_inputs` 互相 set_value 会触发 Change 事件 → 用 `set_input_if_changed` 守卫防无限递归。

### ✅ 已完成：阶段五 — 表格内 Bit 位直接勾选编辑

用户新增需求（本次会话）：

> 表格页直接添加一列，使用 Bit 位显示，直接可以勾选编辑，最多 8 个 bit 位一行，和编辑数据的位显示一样。

| # | 任务 | 说明 | 状态 |
|---|------|------|------|
| 1 | 表列调整 | `RegTableDelegate` 新增 `store: SharedStore` 字段与「位 (Bit)」列（位于「名称」之后），各列宽度重排 | ✅ |
| 2 | 位列渲染 | `render_td` 新增 `"bits"` 分支：寄存器区每行 16 位、**最多 8 位一行**（2 行 × 8）；位区每行 1 个复选框 | ✅ |
| 3 | 实时读值 | 位列直接从共享 store 读实时值（不走快照），勾选后 `window.refresh()` 立即翻转 | ✅ |
| 4 | 直接勾选写回 | `on_click` 计算新值 `set(area,addr,"UI")` 写回 store，修改行写入者标记为 UI | ✅ |
| 5 | 只读区域禁用 | 线圈/保持寄存器可勾选；离散输入/输入寄存器 `.disabled(true)` | ✅ |
| 6 | 唯一 id | 复选框 id 用 `SharedString::from(format!("tblbit-{addr}-{bit_ix}"))` 保证每格唯一 | ✅ |
| 7 | 构建/测试/验证 | `cargo build` 无警告；26 测试全绿 | ✅ |
| 8 | 文档 | `README.md` 补充表格内位编辑说明 | ✅ |

**踩坑记录：**
- `Checkbox::new` 的 `ElementId` 不支持 3 元组 `(&str,usize,usize)`，也不支持 `(ElementId, usize)`（`usize: Into<SharedString>` 不成立）→ 改用 `SharedString::from(format!())` 作为唯一 id。
- `Checkbox::disabled()` 来自 `Disableable` trait，需显式 `use gpui_component::Disableable`。
- 表 `render_td` 的 `match` 各分支需同类型（`Div`）→ 位列的 `if/else` 两分支都不要 `into_any_element()`（内层 `mk_bit` helper 返回 `AnyElement` 作为子元素即可）。
- `render_td` 需带实时值读取，需给 delegate 注入 `SharedStore`（在 `AppView::new` 创建表时 `RegTableDelegate::new(store.clone())`）。

### ✅ 已完成：阶段六 — Modbus 协议(16进制)解析弹框

用户新增需求（本次会话）：

> 主界面添加一个解析 Modbus 协议（16 进制）的弹框，类似数据编辑框：显示功能码，可根据协议内容选择 Int16/Float/String，切换字节序显示对应值，同样显示按位功能。

| # | 任务 | 说明 | 状态 |
|---|------|------|------|
| 1 | 帧解析逻辑 | `protocol/modbus.rs` 新增 `parse_modbus_hex` + `ModbusFrameParse`：解析 MBAP（事务/协议/长度/单元）+ 功能码，用 `length` 字段区分请求/响应并提取寄存器数据字 | ✅ |
| 2 | 解析单元测试 | FC 0x03 响应/请求、0x06、0x10 请求、0x01 线圈、短帧/非法/奇数长度拒绝 | ✅ 全绿 |
| 3 | 解析器状态 | `ParserState`：`words`/`bits`/`value_type`/`byte_order`/`string_chars`/`info`/`error`；`decode_words` 复用类型+字节序解释，`toggle_bit` 按位重算 | ✅ |
| 4 | 解析器实体 | AppView 新增 `parser_state`、`parser_type_select`、`parser_byte_order_select`、`parser_hex_input`、`parser_str_len_input`，默认示例帧预解析 | ✅ |
| 5 | 订阅联动 | hex 输入 `Change` 自动解析；类型/字节序 `Confirm`、String(N) `Change` 更新解析器并 `window.refresh()` 刷新 | ✅ |
| 6 | 弹框渲染 | 主界面「Modbus 解析」按钮打开：hex 输入+解析按钮、MBAP+功能码+结构说明头、类型/字节序/String(N) 选择、解码值+hex 字、按位网格（最多 8 位一行） | ✅ |
| 7 | 构建/测试/验证 | `cargo build` 无警告；31 单测 + 1 集成全绿 | ✅ |
| 8 | 文档 | `README.md` 补充 Modbus 解析弹框说明 | ✅ |

**踩坑记录：**
- `open_dialog` 的内容闭包是 `Fn`（每次渲染重新调用）→ 内部 `move` 闭包不能直接消费外层实体，需先 `let phx = parser_hex_input.clone();` 再移入。
- 帧解析需用 MBAP `length` 字段区分请求（length=6，无数据）与响应（length=3+字节数）以及 0x10 请求/响应。
- 位复选框 id 用 `SharedString::from(format!("parbit-{i}"))` 避免跨渲染冲突。
- 解析器为纯查看/解码器，不写共享 store（与编辑弹窗不同）。

### ✅ 已完成：阶段七 — 解析自动匹配 + Int64/UInt64/Double 类型

用户新增需求（本次会话）：

> 1. 协议解析添加自动匹配字节序按钮：数字类型值需在 0~10000（范围可改）；String 匹配字符解析出有效 ANSI/ASCII（不含中文）。
> 2. 协议解析按返回字节数默认选择类型：2 字节→Int16，4 字节→Int32，8 字节→Int64。
> 3. 添加 Int64/UInt64/Double 数据类型支持。

| # | 任务 | 说明 | 状态 |
|---|------|------|------|
| 1 | 64 位字节序 | `ByteOrder` 新增 `encode_u64` / `decode_u64`（按 32 位半字独立套用字节序），4 寄存器 | ✅ |
| 2 | 数据类型扩展 | `ValueType` 新增 `Uint64` / `Int64` / `Double`（4 寄存器），更新 `ALL`/`label`/`fixed_registers`/`encode_text`/`decode_words`；编辑弹窗与解析器同步支持 | ✅ |
| 3 | 模型单元测试 | `encode_u64`/`decode_u64` 全字节序往返、连线模式、64 位类型字宽、UInt64/Int64/Double 全字节序往返 | ✅ 全绿 |
| 4 | 自动匹配类型 | 解析时按 `words.len()*2` 字节数默认选择（2→Int16、4→Int32、8→Int64），更新解析器状态与类型下拉；另有「自动匹配类型」按钮 | ✅ |
| 5 | 范围输入 | `parser_range_input`（默认 10000）数字类型显示，自动匹配字节序时读取 | ✅ |
| 6 | 自动匹配字节序 | `match_byte_order`/`numeric_value`/`string_bytes`/`valid_ansi_bytes`：数字值 ∈ [0,范围]，String 解析有效 **ANSI/ASCII**（0x20–0x7E，排除中文等多字节；按 ALL 顺序优先 ABCD） | ✅ |
| 7 | 对话框按钮 | 「自动匹配类型」「自动匹配字节序」按钮 + 范围输入 + 规则提示 | ✅ |
| 8 | 构建/测试/验证 | `cargo build` 无警告；35 单测 + 1 集成全绿 | ✅ |
| 9 | 文档 | `README.md` 更新解析器类型与自动匹配说明 | ✅ |

**踩坑记录：**
- 自动匹配逻辑在对话框内联闭包实现（`open_dialog` 闭包无法访问 `&mut AppView`），入参实体需先 clone。
- 预解析/`parse_hex_into_parser` 中在 `parser_state.update` 闭包内再调 `type_select.update` 会双向借用 `cx` → 先算 `vt` 到局部、在 update 闭包外另设类型下拉。
- `encode_u64` 采用「每个 32 位半字独立套字节序」约定：CDAB 高半=字交换、低半=字交换，需与测试预期一致。



### ⏳ 计划中：后续阶段（可选）

- [ ] 扩展 S7 协议（实现 `Protocol` trait 并注册）
- [ ] 数据导出/导入（CSV / JSON 配方）
- [ ] 配方管理（保存/加载一组寄存器值）
- [ ] Float64 / Double、BCD 等更多数据类型

---

## 关键技术决策

### 4 种 Modbus 字节序（32 位，2 个连续寄存器）

设 32 位值字节（MSB→LSB）为 `B3 B2 B1 B0`，寄存器 0（首地址）放高字。

| 顺序 | 寄存器 0 | 寄存器 1 | 说明 |
|------|----------|----------|------|
| ABCD | B3 B2 | B1 B0 | 大端 Big-Endian（默认） |
| CDAB | B1 B0 | B3 B2 | 小端 Little-Endian（字交换） |
| BADC | B2 B3 | B0 B1 | 大端字交换（字内字节交换） |
| DCBA | B0 B1 | B2 B3 | 小端字节+字交换 |

例：`0x12345678` → ABCD `[0x1234, 0x5678]`；CDAB `[0x5678, 0x1234]`；
BADC `[0x3412, 0x7856]`；DCBA `[0x7856, 0x3412]`。
（DCBA 注意：是标准 Modbus 的字节+字交换表，不是简单按字反字节）

### Bit 位编辑

- 16 位：值 = bits[0..16]（bit0 = LSB）
- 32 位：值 = bits[0..32]，经 `ByteOrder.encode_u32` 拆成 2 个寄存器写入 addr 与 addr+1
- 读取同理用 `ByteOrder.decode_u32` 从两个寄存器还原

---

## 测试结果记录

| 时间 | 命令 | 结果 |
|------|------|------|
| 阶段一完成 | `cargo test` | 8 通过 / 0 失败（7 单测 + 1 集成） |
| 阶段一完成 | `cargo clippy --all-targets` | 无警告 |
| 字节序实现后 | `cargo test --lib model::tests` | 7 通过 / 0 失败 |
| 阶段二完成 | `cargo test` | 15 通过 / 0 失败（14 单测 + 1 集成） |
| 阶段二完成 | `cargo clippy --all-targets` | 无警告 |
| 阶段二完成 | GUI 启动 `iot-mock.exe` | 进程存活且 `Responding=True`，截图正常 |
| 阶段三完成 | `cargo test` | 22 通过 / 0 失败（21 单测 + 1 集成） |
| 阶段三完成 | `cargo clippy --all-targets` | 无警告 |
| 阶段三完成 | GUI 启动 `iot-mock.exe` | 进程存活且 `Responding=True` |
| 阶段四完成 | `cargo build` | 无警告 |
| 阶段四完成 | `cargo test` | 26 通过 / 0 失败（25 单测 + 1 集成） |
| 阶段四完成 | `cargo test --lib model::tests` | 18 通过 / 0 失败 |
| 阶段五完成 | `cargo build` | 无警告 |
| 阶段五完成 | `cargo test` | 26 通过 / 0 失败（25 单测 + 1 集成） |
| 阶段六完成 | `cargo build` | 无警告 |
| 阶段六完成 | `cargo test` | 32 通过 / 0 失败（31 单测 + 1 集成） |
| 阶段六完成 | `cargo test --lib protocol::modbus` | 含 6 个新增解析用例 |
| 阶段七完成 | `cargo build` | 无警告 |
| 阶段七完成 | `cargo test` | 36 通过 / 0 失败（35 单测 + 1 集成） |
