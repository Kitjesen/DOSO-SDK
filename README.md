# DOSO SDK

DOSO SDK **0.3.0** 统一面向外部开发者与上层系统，对外主入口为 **`RobotClient`**（兼容别名 `DosoClient`），使用 **gRPC / Protobuf (`doso.v1`)**，默认连接机器狗 **`:5052`**。

无论机器狗采用哪种出厂形态，客户业务代码与多语言封装（Rust / C++17 / Python / Android）**永远只连这一个 `RobotClient :5052`**，无需接触板 A 内部端口 `:13145`（`:13145` 已收口为机内私有接口 `doso_sdk::internal::motion_board::MotionBoardClient`）：

- **纯运控底盘（仅板 A，未安装板 B `LingTu`）**：由板 A 侧 [brainstem-dds-bridge](bridges/brainstem-dds-bridge/src/base_gateway.rs) 提供轻量 `:5052 doso.v1` Base Gateway，完整支持 **L1 200Hz `low_level()` 外控** 与 **L2 `sport()` / `teleop()` / `observations()`**，并在 `system().capabilities()` 中明确标示无板 B 自主导航能力（调用 `maps()` / `navigation()` / `localization()` / `tasks()` 返回 `UNIMPLEMENTED`）。
- **完整智能整机（板 A + 板 B `LingTu`）**：客户连板 B `:5052` 获得全部 **L2 本体 + L3 建图/定位/导航/巡检任务**；板间内部传感器与速度指令经 `CycloneDDS` 通信；当 `System.Handshake` 返回 `low_level_endpoint`（如板 A `:5053`）时，`robot.low_level()` 会自动旁路板 B Python 直连板 A Rust，保证低抖动控制。

```text
【纯运控底盘（无 B 板）】
  应用 (RobotClient) ──gRPC :5052 (doso.v1)──► 板 A Rust Base Gateway ──127.0.0.1:13145──► Brainstem

【完整整机（A + B 双板）】
  应用 (RobotClient) ──gRPC :5052 (doso.v1)──► 板 B LingTu Gateway ◄══CycloneDDS rt/*══► 板 A Rust 边车
         │                                                                                     ▲
         └─────────────── robot.low_level() 按 Handshake 自动直连板 A (:5053) ───────────────────┘
```

## 快速开始

需要 Rust 1.94+ 和 `protoc`。

```text
cargo build --locked
cargo test --workspace --locked
```

多语言与配套工程：

- **Rust SDK**：[src/lib.rs](src/lib.rs) · [src/client/mod.rs](src/client/mod.rs) · [快速入门](docs/usage.md)
- **C-ABI / C++17 / Python / Android**：[ffi/](ffi/include/doso_sdk_ffi.h) · [cpp/](cpp/include/doso/robot_client.hpp) · [python/](python/doso_sdk/client.py) · [android/](android/Cargo.toml)
- **板 A 边车与 Base Gateway**：[bridges/brainstem-dds-bridge/](bridges/brainstem-dds-bridge/src/main.rs)（支持 `--enable-dds false` 纯板 A 无 B 板运行）
- **官方 ONNX RL 执行器**：[rl/doso-rl-runner/](rl/doso-rl-runner/src/runner.rs)（基于 `RobotClient::low_level()` 与 `thunder_16dof_deploy.yaml`）
- **板 A 内部诊断客户端（不作为对客入口）**：[src/internal/motion_board.rs](src/internal/motion_board.rs)

## 目录

```text
DOSO-SDK/
├── src/             RobotClient 对外统一客户端及内部 motion_board 隔离模块
├── proto/           对外整机七域协议 doso.v1
├── ffi/             C-ABI 动态库导出（含无硬件 Mock 测试服务）
├── cpp/             C++17 单头文件 RobotClient
├── python/          Python ctypes RobotClient
├── android/         Android JNI / Kotlin 封装
├── bridges/         板 A 内部 CycloneDDS 桥接与 :5052/:5053 Base Gateway
├── rl/              官方 Sim2Sim / Sim2Real ONNX RL 执行器
├── deploy/          systemd 部署单元与纯底盘/双板环境模板
├── examples/        Rust 示例
├── tests/           Rust gRPC 合同测试
├── docs/            架构、API 手册与验收记录
└── brainstem/       板 A 内部协议镜像与 C++ 内部直连客户端
```

## 文档与验证

- [逐板块功能与验收清单](docs/features.md) · [测试记录与复现命令](TESTING.md)
- [API手册](docs/api/README.md) · [架构与职责](docs/architecture.md) · [V3 演进方案](docs/architecture-v3-dds-migration.md)
- [观测能力](docs/observations.md) · [消息格式](docs/message-catalog.md) · [返回码定义](docs/results.md)
- [直连内部端口说明（内部诊断用）](docs/direct-brainstem.md) · [Brainstem API 参考](docs/api/brainstem.md)

`cargo test --locked` 运行工作区单元测试、106 项 gRPC 合同测试与文档编译检查。
