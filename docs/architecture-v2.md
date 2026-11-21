# DOSO SDK 与 Brainstem 架构讨论图

这张图按 2026-11-20 真机审计结果绘制。实线表示当前已经存在的运行链，虚线表示需要纳入下一版发布的能力。

## 当前运行链

```mermaid
flowchart LR
    App[App / 本地 Rust 应用]
    SDK[BrainstemClient\nDOSO Rust SDK]
    B[Brainstem\n13145\n当前运行入口]
    CAN[CAN 电机总线]
    S[IMU / 关节 / 电机反馈]
    Video[Video Gateway\n5050]
    Camera[Orbbec 相机\nlt-camera + go2rtc]
    Cloud[doso-cloud-link\n只读云状态 / MQTT]
    Doso[DosoClient\n可选 LingTu Gateway 5052]
    Lingtu[LingTu 高层服务\nSLAM / mapd / navd / tasks]

    App --> SDK
    SDK -->|gRPC / HTTP2| B
    B --> CAN
    CAN --> S
    S --> B
    App -->|状态与视频查看| Video
    Video --> Camera
    App -.->|可选，当前未监听| Doso
    Doso -.-> Lingtu
    B -->|只读状态| Cloud
    Cloud -->|MQTT| CloudService[云端状态与许可服务]
```

当前真机监听状态如下。

| 入口 | 当前状态 | 责任 |
|---|---|---|
| `13145` | 运行中（`v0.0.1` 源码已合入 `Damp` 与 `RobotExternalControl`） | Brainstem 原生状态、观测、内置运动与 Layer-2 客户 External 统一入口 |
| `5050` | 运行中 | 相机状态、JPEG、WebRTC 视频入口 |
| `5052` | 未监听 | LingTu 七域高层 Gateway，尚未作为当前真机入口 |
| `13146` | Step 1.3 已挂载 | `LocalStatusService` 本机回环只读 `GetControlStatus` 入口 |
| `doso-cloud-link` | 运行中 | 只读云状态和许可同步 |

## 目标 Brainstem 运控链

高度控制应该进入当前 Brainstem 的单一控制链。它不能以第二个服务、第二个 CAN owner 或独立 `han_dog` 进程运行。

```mermaid
flowchart TB
    Sensors[IMU / 关节 / 电机反馈]
    Obs[Observation Builder\n源时间 / 状态 / 关节观测]
    Brain[Brain / ONNX Policy]
    Input[控制输入]
    Y[YUNZHUO\n物理优先输入]
    E[External\n客户逐关节策略]
    G[gRPC\n站坐 / 速度 / Profile]
    Arbiter[ControlArbiter\n租约 / 优先级 / 抢占规则]
    Height[BodyHeightController\n目标校验 / 50Hz / 过渡]
    Handover[Handover + Gain Blend\n平滑切换 / 重新捕获]
    Profile[ProfileManager\n模型 / 增益 / 高度范围]
    Safety[Safety Output\n限幅 / NaN-Inf / 故障 / 电机门]
    CAN[CAN Motor Transport]
    Readback[状态读回\n当前高度 / 目标高度 / 阶段 / 拒绝原因]

    Sensors --> Obs
    Obs --> Brain
    Y --> Input
    E --> Input
    G --> Input
    Input --> Arbiter
    Arbiter --> Height
    Height --> Brain
    Profile --> Height
    Profile --> Handover
    Brain --> Handover
    Handover --> Safety
    Safety --> CAN
    Safety --> Readback
    Height --> Readback
    Arbiter --> Readback
```

高度控制的单一 owner 是 Brainstem。策略需要遵守以下边界。

- YUNZHUO 的优先级由现有仲裁器决定，普通 gRPC 和 External 不能绕过仲裁器直接写 CAN。
- 高度策略使用 50 Hz 控制周期，输入目标必须经过 Profile 的有效范围校验。
- Profile、模型和模型哈希必须随同一个发布 bundle 管理。
- 电机禁用、控制权丢失、Profile 切换、CMS 状态转换、模型错误和电机故障都要取消或挂起高度过渡。
- RPC 回执只代表请求被接纳。实际高度和过渡完成必须通过状态流读回。

建议的 Brainstem 公共状态字段如下。

```text
active_height
target_height
height_profile
handover_phase
policy_status
model_id
model_sha256
reject_reason
```

## 目标发布边界

```mermaid
flowchart LR
    Source[干净的 Thunder 源码提交]
    Build[单一 ARM64 发布构建]
    Bundle[版本化 bundle\n/bin/thunder\nprofiles / models / recovery]
    Manifest[Release Manifest\ncommit / hash / models / config]
    Unit[单一 Brainstem systemd 配置]
    Runtime[机器狗 active runtime]
    Rollback[一个已验证 rollback bundle]

    Source --> Build --> Bundle --> Runtime
    Bundle --> Manifest
    Unit --> Runtime
    Rollback -.-> Runtime
```

当前机器狗需要从多个历史目录收敛到这个发布边界。现有 active binary 是 `thunder_w5_stairs_gesture_rpc_20261119_31af705`，日志仍加载 `thunder_w5_stairs_motion_slots_20261117_0e0115b` 的 Profile。清理必须先固定这组依赖，再生成一个 canonical systemd override，最后才归档旧 deployment 和 drop-in。

## 两层策略模型

内部策略和购买机器狗客户的策略属于两个生命周期。内部策略随 DOSO 发布 bundle 交付，客户策略由客户自己的程序加载和推理。

```mermaid
flowchart TB
    subgraph DosoRelease[内部 DOSO 发布层]
        Builtin[Builtin Policy\n站起 / 坐下 / 遥操作 / 导航]
        Height[HeightPolicy\n内部高度模型 / 50Hz]
        Bundle[签名 Runtime Bundle\n模型 / Profile / 增益 / 哈希]
        Bundle --> Builtin
        Bundle --> Height
    end

    subgraph Customer[购买机器狗的客户层]
        App[客户应用\nPython / C++ / Rust / Torch / ONNX]
        Model[客户自己的模型文件]
        Infer[客户自己的推理循环]
        Model --> Infer
        App --> Infer
    end

    Brainstem[Brainstem\n唯一 CAN owner]
    Session[External Session\nEnter / Enable / Heartbeat]
    Obs[标准观测\nIMU / 关节 / 电机 / 里程计]
    Targets[逐关节目标\nq / dq / kp / kd / tau\nsequence + input_window]
    Safety[SafetyGate\n限幅 / 超时 / 故障 / 回退]
    Motors[CAN / 电机]

    Builtin --> Brainstem
    Height --> Brainstem
    Infer --> Session --> Brainstem
    Brainstem --> Obs --> Infer
    Infer --> Targets --> Safety --> Motors
    Brainstem --> Safety
```

当前 Rust SDK（`BrainstemExternalClient`）、C++ SDK（`brainstem::ExternalControl`）与 Brainstem `v0.0.1` 服务端（`ExternalService` + `ExternalRuntime` + `HardwareExternal` + `RealJoint` 5 项 MIT `SafetyGate`）均已实现并完成本地单测闭环（待随 `v0.0.1` 发布包部署并验收真机）。原生 External frame 是 16 个关节目标，包含 `q`、`dq`、`kp`、`kd`、`tau`、递增 `sequence` 和 Brainstem 签发的 100 ms `input_window`。SDK 不要求客户使用某一种策略语言或推理框架。[External 协议](../brainstem/protocol/brainstem_api/external.proto) · [Rust 实现](../src/brainstem/mod.rs) · [Brainstem HardwareExternal](../../brainstem/thunder/lib/src/external/hardware_external.dart)

External 会话的客户流程如下。

```text
客户程序加载自己的模型
        ↓
BrainstemClient 连接 13145
        ↓
读取能力、关节布局和标准观测
        ↓
Enter → Enable
        ↓
读取 IMU / 关节 / 电机状态
        ↓
客户模型推理
        ↓
50Hz 逐关节目标 + sequence + input_window
        ↓
Brainstem 验证、限幅和安全输出
        ↓
Disable → Exit → 回到约定的内置安全状态
```

External 模式和 HeightPolicy 需要互斥。External 逐关节模式由客户负责高度和关节目标，内部 HeightPolicy 不能同时写入同一组关节。后续可以增加独立的 External Intent 模式，让客户提交速度和目标高度，由 Brainstem 内部 HeightPolicy 生成关节目标，这属于另一个能力合同。

Unitree 的公开 `unitree_rl_gym` 示例采用客户进程加载 TorchScript 策略、订阅 LowState、计算关节目标并发布 LowCmd。Go2 SportClient 还提供站起、坐下、恢复、速度和部分高度等高层动作。公开资料没有显示一个完整的客户模型仓库或模型生命周期服务。DOSO 可以保留这种客户自带模型的自由度，同时让 Brainstem 保持唯一 CAN owner。[Unitree deploy_real.py](https://github.com/unitreerobotics/unitree_rl_gym/blob/main/deploy/deploy_real/deploy_real.py) · [Unitree SportClient](https://github.com/unitreerobotics/unitree_sdk2/blob/main/include/unitree/robot/go2/sport/sport_client.hpp)

## 讨论时需要先定的三个决策

1. BodyHeight 的正式输入是否统一为 `target_height` 加当前速度输入 `[vx, vy, wz]`。
2. 高度策略的状态读回是否直接加入 `RobotControl`，还是随 `ListenCmsState` 扩展。
3. 第一版发布是否只支持当前 Thunder 型号和当前已验证模型，其他型号通过能力查询拒绝。

相关清理证据见 [CLEANUP_PLAN.md](../../DOSO/artifacts/robot-cloud/fs-audit-20261120/CLEANUP_PLAN.md)，高度策略分析见 [bodyheight-architecture-20261120.md](../../DOSO/artifacts/robot-cloud/bodyheight-architecture-20261120.md)。
