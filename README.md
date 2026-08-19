# IoT 协议模拟器 (IoT Protocol Simulator)

一个基于 **Rust + [GPUI](https://gpui.rs) + [gpui-component](https://longbridge.github.io/gpui-component)** 的桌面端
协议仿真工具。目前内置 **Modbus TCP** 服务端模拟，并采用**可扩展架构**，可轻松添加 S7、OPC-UA、MQTT 等其它协议。

## 功能特性

- **Modbus TCP 服务端模拟**：从零实现的 Modbus TCP（MBAP + PDU），支持常用功能码：
  | 功能码 | 名称 | 数据区 |
  |--------|------|--------|
  | 0x01 | 读线圈 | 线圈 Coils |
  | 0x02 | 读离散输入 | 离散输入 |
  | 0x03 | 读保持寄存器 | 保持寄存器 |
  | 0x04 | 读输入寄存器 | 输入寄存器 |
  | 0x05 | 写单个线圈 | 线圈 |
  | 0x06 | 写单个寄存器 | 保持寄存器 |
  | 0x0F | 写多个线圈 | 线圈 |
  | 0x10 | 写多个寄存器 | 保持寄存器 |
- **Modbus 协议解析 (16 进制) 弹框**：主界面「Modbus 解析」按钮打开解析弹框，粘贴完整 Modbus TCP 帧（MBAP + PDU）十六进制即可解析并显示 **功能码**；支持选择 **Int16 / UInt16 / Int32 / UInt32 / Float32 / Int64 / UInt64 / Double / String** 类型并**切换 4 种字节序**，显示对应字节序下的值，同时可**按位显示 / 勾选**（最多 8 位一行）。另有：
  - **自动匹配类型**：按返回字节数自动选择（2 字节→Int16，4 字节→Int32，8 字节→Int64）；
  - **自动匹配字节序**：数字类型匹配值在 `[0, 范围]` 内（范围可编辑，默认 10000）的字节序；String 匹配可解析出有效 **ANSI/ASCII** 字符（不含中文等多字节字符）的字节序。
- **实时数据显示**：UI 每 200ms 刷新，被改动/写入的行会高亮标记，支持切换四个数据区（线圈 / 离散输入 / 保持寄存器 / 输入寄存器）。
- **表格内 Bit 位直接勾选编辑**：数据表新增「位 (Bit)」列，直接显示并为每个寄存器以 **最多 8 位一行** 的方式勾选切换比特（与编辑弹窗的位显示一致）；线圈 / 保持寄存器等可写区域可直接勾选，只读区域禁用。勾选后立即写入共享存储。
- **实时修改数据（Bit 位 / 16 进制编辑）**：双击可写数据区的任意行，弹出 **编辑弹窗**，可在 **位编辑** 与 **16 进制** 两种显示方式间切换：
  - **位编辑**：用 CheckBox 按位勾选（16 位单寄存器 / 32 位双寄存器 / 字符串按占用寄存器的全部位）；每行 8 位，bit0 = 最低位；
  - **16 进制编辑**：按寄存器字输入 16 位十六进制值（空格分隔，如 `1234 5678`），可精确写入原始数据；
  - 实时 **十六进制 / 十进制预览**，确认后立即写入共享存储并刷新表格。
- **字符串 String(N)**：可选数据类型 **String** 并设置字符数 / 字节数 `N`（如 `String(7)`）。占用寄存器数为 `ceil(N/2)`，位与 16 进制视图均按计算出的字节 / 寄存器长度展示，超长内容会给出提示。
- **4 种 Modbus 字节序**：在协议卡的 **启动 Server 前**通过下拉选择 32 位数据的字节序：
  `ABCD (大端)` / `CDAB (小端)` / `BADC (大端字交换)` / `DCBA (小端字节+字交换)`。
  编辑弹窗按所选字节序对 32 位值进行解码/编码。
- **自动模拟**：可开关。开启后每 200ms 自动随机更新若干寄存器，方便观察实时效果。
- **协议控制面板**：启动 / 停止服务、修改监听端口、选择字节序、查看运行状态、连接数、请求数、写入单元数、错误响应数。
- **可扩展架构**：所有协议实现统一的 [`Protocol`](src/protocol/mod.rs) trait，添加新协议只需实现 trait 并注册，UI 自动生成控制卡片。
- **浅色 / 深色主题**一键切换，标题栏提供关闭窗口按钮，并支持拖动自定义标题栏移动窗口。

## 快速开始

```bash
cargo run --release
```

启动后在左侧面板中点击 **启动** 即可在 `127.0.0.1:502` 上开启 Modbus TCP 服务（可用任意第三方 Modbus 客户端连接）。

## 架构

```
src/
├── lib.rs                # 库入口（协议与数据模型，可脱离 UI 复用/测试）
├── main.rs               # 桌面应用入口（GPUI）
├── app.rs                # GPUI 视图：标题栏 / 侧栏 / 数据表 / 弹窗 / 状态栏
├── model.rs              # 数据结构：RegisterStore（共享寄存器区）、快照、模拟
└── protocol/
    ├── mod.rs            # Protocol trait、ServerStats、ProtocolContext（协议抽象层）
    └── modbus.rs         # Modbus TCP 服务端实现（含单元测试）
tests/
└── modbus_tcp.rs         # 端到端集成测试：真实 TCP 客户端 ↔ 服务器 ↔ 共享存储
```

### 数据共享模型

`RegisterStore` 是唯一数据源，通过 `Arc<RwLock<...>>` 在以下各方之间共享：

- Modbus TCP 服务器（后台 tokio 运行时）
- GPUI 界面（UI 线程）

因此客户端写入的数据 UI 立即可见，UI 修改的数据客户端立即可读，多协议之间数据也天然互通。

### 扩展新协议（如 S7）

1. 新建 `src/protocol/s7.rs`，实现 [`Protocol`](src/protocol/mod.rs) trait（`start` / `stop` / `state` / `port` / `set_port` 等）；
2. 在 [`AppView::new`](src/app.rs) 的 `protocols` 列表中注册：
   ```rust
   let protocols = vec![
       ProtocolCard::new(Box::new(ModbusTcpServer::new(DEFAULT_PORT))),
       ProtocolCard::new(Box::new(S7Server::new(0))),
   ];
   ```
3. 侧栏自动生成 S7 控制卡片，其数据同样写入共享 `RegisterStore`。

## 测试

```bash
cargo test
```

- 单元测试：PDU 解析、功能码处理、位打包、越界/非法异常码、读写往返、统计峰值。
- 集成测试：在临时端口启动真实服务器，用原始 TCP 客户端完成读写验证并断言共享存储与统计值。

## 依赖

- `gpui` / `gpui-component` / `gpui-component-assets`：桌面 UI
- `tokio`：Modbus TCP 服务器异步运行时
- `anyhow` / `log` / `env_logger`：错误处理与日志

## License

Apache-2.0
