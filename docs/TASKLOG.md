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
