# DOSO-SDK

**DOSO 四足机器人统一运行时与开发套件（`v0.0.1`）** —— 基于 Rust 实时底座 `doso-runtime`，提供上层统一控制客户端 `RobotClient`（`:5052`）、策略热更新（`:5054`）、硬件安全门控与公网云端 OTA 能力。

---

## 核心特性

- **单包集中式架构（`doso-runtime v0.0.1`）**：核心运行时收敛于 `runtime/Cargo.toml`，按六大职责模块清晰划分，`safety/gate.rs` 严格启用 `#![forbid(unsafe_code)]`，覆盖 `46` 项单元与集成测试，Windows 与 RDK S100P ARM64 均通过零告警 `clippy`。
- **确定性实时控制（`200Hz/50Hz BrainEngine`）**：在 RDK S100P（`socketcan`）实测 `max_mutex_wait_us = 10 µs`、`max_tick_us = 262 µs`、`max_jitter_us = 245 µs`，策略热更期间控制进程 `0` 重启。
- **纯本地 ONNX / 策略包导出与黄金比对**：内置 [export_doso_policy.py](tools/export_doso_policy.py) 与 `doso-cli`，支持单模型与策略包目录 `inspect`（输出 `posture:`）、`deploy`、`pack`、`push` 与 `rollback`。
- **公网云端 OTA 协同**：协同阿里云上海服务（`https://api-test.dosorobotics.com/ota`）支持 `policy_package`（热更）、`brainstem_release`（底座升级）与 Android APK 分发，已在 `.13` Dog3 真机验证 `2.03 s` 零重启推送成功。

---

## 仓库版本矩阵（Aligned `v0.0.1`）

| 仓库 | 默认基线分支 | Tag | 定位 |
| :--- | :--- | :--- | :--- |
| `Kitjesen/DOSO-SDK` | `rust` | `v0.0.1` | 机载运行时 `doso-runtime`、策略工具链、SDK 与运维文档 |
| `Kitjesen/robot-cloud` | `main`（私有） | `v0.0.1` | 公网云端 OTA 控制面（`doso-ota`、`doso-ota-release`、`/v1/apps/android/*`、`/v1/devices/{id}/status`） |
| `Kitjesen/brainstem` | `master` | `v0.0.1` | 底层硬件桥接与运动控制参考基线 |

---

## 模块与目录总览

`doso-runtime`（[runtime/README.md](runtime/README.md)）将核心能力解耦为六个职责模块及统一配置文件 [thunder_v4.yaml](runtime/config/thunder_v4.yaml)：

| 路径 | 职责说明 |
| :--- | :--- |
| `runtime/src/hardware/` | 硬件总线与执行器/IMU 抽象（CAN / `socketcan`、仿真适配） |
| `runtime/src/arbiter/` | 控制权仲裁与命令通道优先级调度 |
| `runtime/src/control/` | `200Hz/50Hz` 运动状态机、步态切入融合、机身高度与跌倒恢复 |
| `runtime/src/policy/` | ONNX 策略加载、Golden/Shadow 一致性校验与多槽位（`slots`）热切换 |
| `runtime/src/safety/` | `#![forbid(unsafe_code)]` 安全门控（`SafetyGate`）与崩溃急停下电 |
| `runtime/src/server/` | 服务外壳（`:5052` 网关、`:13145` gRPC、`:13146` `LocalStatusGrpcServer`、`:5054` 热部署与看门狗） |
| `runtime/src/bin/` | `doso-brainstem` 主守护进程与 `doso-cli` 本地控制命令行工具 |
| `tools/` | [export_doso_policy.py](tools/export_doso_policy.py) 策略检查、打包、热更推送与回滚脚本 |
| `docs/` | 架构设计、API 手册、真机验收矩阵与公网云端 OTA 指南 |

---

## 四套实测策略矩阵（RDK S100P ARM64）

以下为四套主干策略在 RDK S100P ARM64 上的实测 Golden 误差与推理预热/运行耗时：

| 策略名称 | Golden 最大误差（`golden_max_err`） | ARM64 实测推理耗时（`warmup_us` / P50） |
| :--- | :--- | :--- |
| `flat_w5` | `3.73e-7` | `43–49 µs` |
| `single_frame_height_v2` | `3.73e-7` | `42 µs` |
| `stairs_cts_v39` | `2.98e-6` | `394–424 µs` |
| `recovery_v89` | `6.56e-7` | `79–88 µs` |

---

## 快速开始

### 1. 构建、静态检查与测试

```bash
# 运行离线 Clippy 严格检查（零告警）
cargo clippy --manifest-path runtime/Cargo.toml --all-targets --offline -- -D warnings

# 运行全部 46 项单元与集成测试
cargo test --manifest-path runtime/Cargo.toml --offline

# 构建发布版可执行文件
cargo build --manifest-path runtime/Cargo.toml --release --offline
```

### 2. 启动 `doso-brainstem` 与健康检查

```bash
# 指定硬件配置启动机载运行时（默认内嵌 thunder_v4.yaml）
cargo run --manifest-path runtime/Cargo.toml --bin doso-brainstem -- --hardware-config runtime/config/thunder_v4.yaml

# 检查热部署端口（:5054）健康与策略槽位状态
curl -sS http://127.0.0.1:5054/health
```

### 3. 策略检查、打包与本地热更

```bash
# 检查 ONNX 模型或策略包目录（输出包含 posture: 标识）
python tools/export_doso_policy.py inspect <POLICY_SOURCE_DIR_OR_ONNX>

# 一键生成策略包 + Golden 并推送到机器狗（:5054）
python tools/export_doso_policy.py deploy <CHECKPOINT_ONNX> --slot flat --admin-url http://<ROBOT_IP>:5054

# 或分步打包、推送与回滚
python tools/export_doso_policy.py pack --onnx <CHECKPOINT_ONNX> --name <NAME> --template height --out-dir dist/<PACKAGE_DIR>
python tools/export_doso_policy.py push --pkg-dir dist/<PACKAGE_DIR> --admin-url http://<ROBOT_IP>:5054 --slot flat
python tools/export_doso_policy.py rollback --admin-url http://<ROBOT_IP>:5054 --slot flat
```

### 4. 公网云端 OTA 推送

完整端到端 SOP、OSS 上传与三通道接口规范（`policy_package` / `brainstem_release` / `android_apk`）及 `.13` Dog3 真机实测复盘请查阅：

- Markdown 在线指南：[docs/OTA_CLOUD_PUSH_GUIDE.md](docs/OTA_CLOUD_PUSH_GUIDE.md)
- 排版归档 PDF：[docs/DOSO_OTA_Cloud_Push_Guide.pdf](docs/DOSO_OTA_Cloud_Push_Guide.pdf)

---

## 文档导航

### 云端运维与 OTA
- [OTA_CLOUD_PUSH_GUIDE.md](docs/OTA_CLOUD_PUSH_GUIDE.md) — 公网云端 OTA 六步推送流程、三通道设计及 `.13` 真机实测记录
- [DOSO_OTA_Cloud_Push_Guide.pdf](docs/DOSO_OTA_Cloud_Push_Guide.pdf) — 同主题可打印/分享的四页排版 PDF 文档
- [release.md](docs/release.md) — 版本发布规范与对齐说明

### 运行时、架构与传输
- [runtime/README.md](runtime/README.md) — `doso-runtime` 编译、模块结构与配置参数说明
- [architecture.md](docs/architecture.md) — SDK 总体分层架构
- [architecture-v3-dds-migration.md](docs/architecture-v3-dds-migration.md) — 架构演进与桥接迁移说明
- [direct-brainstem.md](docs/direct-brainstem.md) — 直连 `brainstem` 协议与实机部署说明
- [deploy-board-a-shm.md](docs/deploy-board-a-shm.md) — 板端共享内存与部署参考

### SDK 使用、API 与开发教程
- [usage.md](docs/usage.md) — 控制接口接入与典型使用方式
- [features.md](docs/features.md) — 功能特性清单
- [api/README.md](docs/api/README.md) 与 [api/brainstem.md](docs/api/brainstem.md) — 核心 API 与底层消息说明
- [observations.md](docs/observations.md) 与 [message-catalog.md](docs/message-catalog.md) — 观测空间维度与消息目录
- [tutorial-teleop-and-rl.md](docs/tutorial-teleop-and-rl.md) — 遥操作与强化学习联调教程

### 测试与验收基线
- [TESTING.md](TESTING.md) — 工程测试指南与回归要求
- [results.md](docs/results.md) — 历史与基线测试结果汇总
- [REAL_ROBOT_ACCEPTANCE_MATRIX.md](docs/REAL_ROBOT_ACCEPTANCE_MATRIX.md) — 真机实测验收矩阵
