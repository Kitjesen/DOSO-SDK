# DOSO-SDK

**DOSO 机器人统一开发套件（SDK、协议、多语言绑定与桥接组件）** —— 为上层应用、遥控端、自主导航（LingTu）与外部 RL 策略提供统一的 Rust / Python / C++ / Android 控制客户端与板间通信能力。

> 机载实时控制底座（`doso-brainstem` / `doso-cli`）、策略包（`models/packages/`）与板端部署配置统一维护在 [`Kitjesen/brainstem`](https://github.com/Kitjesen/brainstem)。

---

## 目录结构

| 路径 | 职责说明 |
| :--- | :--- |
| [`doso-sdk/`](doso-sdk/) | Rust `doso-sdk` 核心客户端库（`RobotClient`、七大公开领域与 `brainstem.api.v1` 适配） |
| [`proto/`](proto/) | 统一 Protobuf 协议定义：公开协议 `proto/doso/` 与底层协议 `proto/brainstem/` |
| [`bindings/`](bindings/) | 多语言绑定：`ffi/`（C ABI）、`python/`、`cpp/`、`android/`（JNI）、`brainstem-cpp/`（独立 C++ gRPC 客户端） |
| [`bridges/`](bridges/) | 板端桥接组件：`brainstem-dds-bridge`（`:5052` Base Gateway / DDS）与 `doso-shm-ipc`（无锁共享内存 IPC） |
| [`rl/`](rl/) | `doso-rl-runner`：面向 `RobotClient::low_level()` / SHM 的外部 ONNX 强化学习策略运行器 |
| [`scripts/`](scripts/) | 协议代码生成脚本（`generate_grpc.py` 及单测） |
| [`docs/`](docs/) | 架构设计、API 手册、使用教程与 OTA 文档 |

---

## 快速开始

### 1. Rust Workspace 检查与测试

```bash
# 格式化与严格 Clippy 检查
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# 运行 Rust SDK、桥接与 RL Runner 测试
cargo test -p doso-sdk
cargo test -p doso-shm-ipc -p doso-sdk-android
cargo test -p brainstem-dds-bridge -p doso-rl-runner -- --test-threads=1
```

### 2. FFI 与 Python SDK 测试

```bash
# 编译带 mock 的 FFI 动态库并运行 Rust FFI 测试
cargo build -p doso-sdk-ffi --features test-mock
cargo test -p doso-sdk-ffi --features test-mock

# 运行 Python SDK 集成测试
python -m unittest discover -s bindings/python/tests
```

### 3. Android JNI 编译

```bash
# Linux / macOS
bash bindings/android/build.sh

# Windows
pwsh bindings/android/build.ps1
```

### 4. 生成 LingTu Python gRPC 绑定

```bash
python scripts/generate_grpc.py --lingtu <LINGTU_REPO_DIR>
python -m unittest scripts/test_generate_grpc.py
```

---

## 文档导航

- [使用说明](docs/usage.md)
- [架构总览](docs/architecture.md)
- [协议消息目录](docs/message-catalog.md)
- [遥控与 RL 教程](docs/tutorial-teleop-and-rl.md)
- [直连 Brainstem 指南](docs/direct-brainstem.md)
- [板端共享内存参考](docs/deploy-board-a-shm.md)
- [云端 OTA 推送指南](docs/OTA_CLOUD_PUSH_GUIDE.md)
