# DOSO Brainstem Runtime (`doso-runtime`)

`runtime/` 采用**单一职责驱动 Rust 包**（`doso-runtime`）+ **独立硬件参数 YAML 配置**（`config/thunder_v4.yaml`），将底层驱动、控制权仲裁、步态状态机、声明式策略推理、硬件安全门控与单进程服务外壳清晰解耦。

## 目录结构

```text
runtime/
├── Cargo.toml                  # 单包 doso-runtime + 两个可执行文件 (doso-brainstem / doso-cli)
├── config/
│   └── thunder_v4.yaml         # 机器人硬件拓扑、总线映射、电机协议限幅、默认站姿/限位与安全阈值（唯一数据源）
├── deploy/                     # 板端 systemd 服务与环境配置模板
│   ├── doso-brainstem.service
│   ├── rdk_s100_hardware.env
│   ├── rdk_s100_bench.env
│   └── switch_runtime.sh
├── src/
│   ├── lib.rs                  # 根包启用 #![deny(unsafe_code)]，仅 FFI 模块显式放行
│   ├── params.rs               # 加载并校验 RobotHardwareParams（内置 thunder_v4.yaml，支持文件覆盖）
│   ├── types.rs                # 零 IO 共享数据结构（JointVec16 / MitCmd16 / ProprioFrame / WalkCommand）
│   ├── hardware/               # 物理设备驱动与线缆协议编解码
│   │   ├── mod.rs              # HalBackend 抽象、HalDiagnostics、CanBusFrame
│   │   ├── robostride.rs       # RS04/RS02 29-bit CAN 编解码
│   │   ├── socketcan.rs        # sendmmsg/recvmmsg 批量收发、84 帧上电序列、Vbus 与确认下电
│   │   ├── hipnuc.rs           # Hi91 CRC16 流式解码与四元数重力投影
│   │   ├── serial_imu.rs       # Linux termios IMU 串口接收线程
│   │   ├── yunzhuo.rs          # YunZhuo SUBS/SBUS 原始帧解码
│   │   ├── remote_dev.rs       # Linux /dev/yunzhuo 与 /dev/input/js* 设备读取
│   │   └── bench.rs            # BenchNullHal（空载/仿真）与 ReplayHal（离线回放）
│   ├── arbiter/                # 控制权仲裁与遥控器模式门控（零 IO、纯可移植逻辑）
│   │   ├── control_arbiter.rs  # 物理遥控绝对抢占 vs gRPC 租约与单调序号校验
│   │   └── yunzhuo_gate.rs     # CH5 三档门控、斜坡加减速、死区解锁与手柄映射
│   ├── control/                # 运动状态机与技能编排
│   │   ├── fsm.rs              # 200Hz/50Hz BrainEngine、FsmState、SpeedMode 与历史快照
│   │   ├── handover.rs         # 平滑步态切入混合与机身高度限速控制器
│   │   ├── gestures.rs         # 5 个内置动作与关键帧播放器
│   │   └── recovery.rs         # 11 阶段跌倒恢复状态机
│   ├── policy/                 # 声明式策略图与零分配推理引擎
│   │   ├── blocks.rs           # 观测/输出积木表（支持简写、posture/kinematics 别名与多顺序预设）
│   │   ├── compiler.rs         # 零分配 f64 环形历史与双向 joint_order 编译图
│   │   ├── ort.rs              # 预绑定零分配 OrtValue 与单例 C-API 推理后端
│   │   └── package.rs          # SHA256 校验、Golden 门禁、Shadow 离线评估与策略槽位注册表
│   ├── safety/                 # 不可绕过的硬件安全门控与崩溃急停
│   │   ├── gate.rs             # #![forbid(unsafe_code)] SafetyGate 与 SafetyConfig
│   │   └── cleanup.rs          # 全局 Panic 钩子、紧急阻尼下电与 SIGINT/SIGTERM 退出处理
│   ├── server/                 # 网络与守护进程外壳
│   │   ├── grpc.rs             # SharedBrain、无锁争用 TelemetrySnapshot 与 gRPC 服务实现
│   │   ├── hot_deploy.rs       # :5054 锁外 Shadow 策略热更新与槽位回滚路由
│   │   └── watchdog.rs         # 原生 Linux NOTIFY_SOCKET systemd 看门狗心跳
│   └── bin/
│       ├── doso-brainstem.rs   # 单进程主控守护进程（支持 --hardware-config / DOSO_HARDWARE_CONFIG）
│       └── doso-cli.rs         # 策略查验与热部署命令行工具（list / inspect / validate / push / rollback）
└── tests/                      # 按职责划分的 6 组集成与端到端测试
```

## 模块职责边界

| 模块 | 核心职责 | 约束 |
| :--- | :--- | :--- |
| `params` + `config/thunder_v4.yaml` | 硬件拓扑、CAN 总线、串口设备、电机限幅、默认站姿/限位与安全参数 | 启动强制双射与边界校验，支持 `--hardware-config` 覆盖 |
| `types` | 16 自由度关节向量、MIT 目标与本体感知帧 | 零 IO、零 `unsafe` |
| `hardware` | CAN/串口协议编解码与后端驱动 | 隔离所有 Linux 设备句柄，仅在 Linux 驱动子模块放行 `unsafe` |
| `arbiter` | 控制权租约仲裁与遥控三档门控状态机 | 零 IO、纯确定性逻辑，与 Linux 设备读取彻底分离 |
| `control` | 200Hz 主环、步态切换、机身高度、内置动作、跌倒恢复 | 仅通过 `SafetyGate` 与 `HalBackend` 交互 |
| `policy` | 策略 YAML 解析、`f64` 历史缓存、Golden/Shadow 门禁与 ORT 推理 | `ort.rs` 预绑定 I/O，热路径零堆分配 |
| `safety` | 硬件限幅、姿态倾角、心跳时效校验与崩溃阻尼下电 | `gate.rs` 强制 `#![forbid(unsafe_code)]` |
| `server` | `:5052` 网关、`:13145` 内部 gRPC、`:5054` 热更新与 systemd 看门狗 | 遥测读路径走 `RwLock<TelemetrySnapshot>`，零 RT 互斥锁争用 |

## 检查与测试

```powershell
$env:CYCLONEDDS_ROOT='off'
$env:ONNXRUNTIME_DLL_PATH='C:\Users\99563\miniconda3\Lib\site-packages\onnxruntime\capi\onnxruntime.dll'
cargo clippy --manifest-path runtime/Cargo.toml --all-targets --offline -- -D warnings
cargo test --manifest-path runtime/Cargo.toml --offline -- --nocapture
```
