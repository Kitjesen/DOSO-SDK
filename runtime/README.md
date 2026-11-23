# DOSO Brainstem Runtime

`runtime/` 采用扁平化两层 Rust Workspace 结构（`runtime/<module>/src/<file>.rs`），统一承载底层控制核心库、可执行服务与板端部署配置。

## 目录结构

```text
runtime/
├── Cargo.toml
├── core/                   # L0 (doso-core) 零 IO 基础类型、运动学与 FSM 仲裁
├── hal/                    # L1 (doso-hal) 扁平化协议编解码与硬件后端驱动
│   └── src/                # traits / robostride / hipnuc / yunzhuo / bench / replay / socketcan / serial_imu / remote_input
├── safety/                 # L2 (doso-safety) 唯一硬件下发门控 SafetyGate 与阻尼保护
├── policy/                 # L3 (doso-policy) 声明式策略图编译、f64 历史观测与 Golden/Shadow 校验
├── brain/                  # L4 (doso-brain) 200Hz 主控制引擎与 11 阶段跌倒恢复
├── brainstem/              # (doso-brainstem) 单进程主控服务（gRPC / 热更新 / 看门狗 / 紧急阻尼下电）
├── cli/                    # (doso-cli) 策略热部署与回滚命令行工具
└── deploy/                 # 扁平化板端部署与环境配置
    ├── doso-brainstem.service
    ├── rdk_s100_hardware.env
    ├── rdk_s100_bench.env
    └── switch_runtime.sh
```

## 模块职责

### 核心库（L0–L4 解耦层）

| 层级 | 目录 | Crate 名称 | 核心职责 |
| :--- | :--- | :--- | :--- |
| **L0** | `core` | `doso-core` | 零 IO 确定性基础类型、四足轮腿运动学常量、FSM 状态定义与控制租约仲裁 |
| **L1** | `hal` | `doso-hal` | 硬件抽象层：`robostride.rs` / `hipnuc.rs` / `yunzhuo.rs` 协议编解码，以及 `bench.rs`、`replay.rs`、`socketcan.rs`、`serial_imu.rs`、`remote_input.rs` 后端驱动 |
| **L2** | `safety` | `doso-safety` | 唯一硬件下发门控 `SafetyGate`、关节指令包络校验、本体状态健康检查与故障阻尼保护 |
| **L3** | `policy` | `doso-policy` | 声明式策略图编译、纯 `f64` 零分配历史观测环形缓冲、SHA256 与 Golden 轨迹门禁、Shadow 评估及动态推理后端 |
| **L4** | `brain` | `doso-brain` | 200Hz 主控制引擎，编排步态与速度模式（`SpeedMode`）、动作库、机身高度、11 阶段跌倒恢复、物理遥控闭环、仿真步进与零位标定 |

### 可执行程序

- **`brainstem` (`doso-brainstem`)**：单进程主控服务，集成实时控制环、对外网关与内部 gRPC（`brainstem_grpc.rs`）、零重启策略热更新（`hot_deploy.rs`）、systemd `Type=notify` 看门狗心跳（`watchdog.rs`）以及 Panic/信号紧急阻尼下电保护（`fatal_cleanup.rs`）。
- **`cli` (`doso-cli`)**：面向研发与运维的轻量级命令行工具，支持离线策略包校验（`validate`）、在线热推送（`push`）及槽位即时回滚（`rollback`）。

### 部署配置（`deploy/`）

- **`deploy/doso-brainstem.service`**：车载计算单元 systemd 服务配置（`Type=notify`、`WatchdogSec=3s` 实时守护）。
- **`deploy/rdk_s100_hardware.env` / `deploy/rdk_s100_bench.env`**：实机 SocketCAN 模式与桌面空载测试台模式环境参数模板。
- **`deploy/switch_runtime.sh`**：RDK S100P 板端一键切换脚本，支持在 `rust` 与 `dart` 运行时间原子切换软链、启停 systemd 服务并校验网关 `:5052` 连通性。

## 本地检查与测试

```powershell
cargo clippy --manifest-path runtime/Cargo.toml --all-targets --offline -- -D warnings
cargo test --manifest-path runtime/Cargo.toml --offline
```
