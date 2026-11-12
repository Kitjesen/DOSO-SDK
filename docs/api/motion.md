# Motion：内置与外部运控

`robot.motion()` 提供姿态请求、电机输出与profile管理、外部逐关节控制、停止、受控速度输入，以及身体、控制和定位运动反馈。所有方法复用 `DosoClient` 的 gRPC 通道与认证配置。

`DosoClient::connect()` 完成只读握手。运动权限由机器人端仲裁，可显式申请，也可在首条有效速度输入时申请 native authority。Brainstem 的硬件控制 lease 由 `lingtu-driver` 持有。`control_state()` 读取 native 操作者权限，`control_status()` 读取 Brainstem 源端控制状态。

连接配置见[接入说明](../usage.md)，错误与接纳规则见[返回码和状态](../results.md)，消息字段见[消息目录](../message-catalog.md)。

## 接口索引

| 功能 | 方法 | Rust实现 |
| --- | --- | --- |
| 站起与坐下 | [stand_up](#stand_up)、[sit_down](#sit_down) | [posture.rs](../../src/motion/posture.rs) |
| 电机与策略 | [enable_motors](#enable_motors)、[disable_motors](#disable_motors)、[profile](#profile)、[switch_profile](#switch_profile) | [motors.rs](../../src/motion/motors.rs)、[profile.rs](../../src/motion/profile.rs) |
| 自定义控制器 | [controller](#controller)、[external_control](#external_control)、[外部会话方法](#外部会话方法) | [external.rs](../../src/motion/external.rs) |
| 停止、复位与恢复 | [stop](#stop)、[emergency_stop](#emergency_stop)、[clear_emergency_stop](#clear_emergency_stop)、[resume](#resume) | [stop.rs](../../src/motion/stop.rs)、[resume.rs](../../src/motion/resume.rs) |
| 身体与控制状态 | [body_state](#body_state)、[watch_body_state](#watch_body_state)、[control_status](#control_status)、[control_state](#control_state)、[watch_control_state](#watch_control_state) | [body.rs](../../src/motion/body.rs)、[control.rs](../../src/motion/control.rs) |
| 运动诊断快照 | [status](#status)、[watch_state](#watch_state)、[watch_state_at](#watch_state_at) | [status.rs](../../src/motion/status.rs)、[watch.rs](../../src/motion/watch.rs) |
| 定位运动反馈 | [velocity](#velocity)、[watch_velocity](#watch_velocity)、[odometry](#odometry)、[watch_odometry](#watch_odometry) | [velocity.rs](../../src/motion/velocity.rs)、[odometry.rs](../../src/motion/odometry.rs) |
| 控制权管理 | [claim_control](#claim_control)、[claim](#claim)、[会话 control_state](#会话-control_state)、[release_control](#release_control) | [teleop.rs](../../src/motion/teleop.rs) |
| 受控遥操作 | [teleop](#teleop)、[publish_velocity](#publish_velocity)、[send_velocity](#send_velocity)、[latest_reply](#latest_reply)、[hold](#hold)、[close](#close) | [teleop.rs](../../src/motion/teleop.rs) |

## 共同约定

以下签名中的 `rpc` 为 `doso_sdk::rpc`，`Result<T>` 为 `doso_sdk::Result<T>`。主要返回类型也从 crate 根导出。

| 项目 | 约定 |
| --- | --- |
| `request_id` | `Option<&str>`。`None` 由 SDK 生成 26 字符 ULID，`Some` 须为非空白文本。每次新操作使用独立 ID |
| 单次读取和普通控制 | 使用 `Config.timeout`，默认 10 秒 |
| 姿态、电机与profile写操作 | 使用 `max(Config.timeout, 15 秒)`，覆盖native事务与driver ACK等待 |
| 长期订阅 | 沿用 API key，无普通单次请求的 grpc-timeout。调用者管理订阅时长 |
| 调用错误 | 返回 SDK `Error`，通过 `code()`、`reason_code()`、`request_id`、`submission_unknown` 判断 |
| 流内错误 | `stream.message().await` 返回 `tonic::Status` |
| 订阅释放 | 丢弃状态或反馈流会释放订阅。运动停止使用停止接口或遥操作会话的 hold/close |

`CommandReceipt` 的主要字段如下。`Ok(receipt)` 后仍需检查接纳与原因。

| 字段 | 含义 |
| --- | --- |
| `request_id` | 与请求对应的身份，SDK 校验返回身份 |
| `accepted` | `Some(true)` 为明确接纳，`Some(false)` 为明确拒绝，`None` 为缺少接纳信息 |
| `ok` | 回执结果投影，与 `accepted` 的关系按公共回执合同处理 |
| `stage` | 当前确认阶段，姿态常见为 `posture_requested` |
| `reason`、`reason_code` | owner 返回的说明与机器原因，可能为空 |
| `details` | 可选原始补充信息，包括 native ACK |

动作接纳后，通过身体状态核查姿态到位。停止回执表达对应软件控制链的确认，物理停稳还需设备状态证据。写操作超时或回执未知时保留请求 ID，核查状态后再决定下一步。

## 站起与坐下

### stand_up

```text
pub async fn stand_up(&self, request_id: Option<&str>) -> Result<CommandReceipt>
```

请求从当前允许的身体状态进入站立，返回姿态命令接纳回执。机器人端经过命令 journal、运动 guard、nav 仲裁与 driver 检查，必要时先完成停止屏障。

前置条件包括 native 姿态能力已接入、电机输出允许、无关键电机故障、源端控制权允许。活动导航任务、其他操作者权限或身体过渡可能使请求被拒绝。以 `BodyKind::Standing` 等后续源状态核查到位。

### sit_down

```text
pub async fn sit_down(&self, request_id: Option<&str>) -> Result<CommandReceipt>
```

请求坐下，参数与回执使用共同约定。有 walking lease 的路径需先确认停止，再向身体控制器提交坐下动作。前置条件和常见失败与站起相同，以后续 `BodyKind::Grounded` 核查到位。

两个方法的 ACK 未知会返回 `UNAVAILABLE`，可携带 `posture_ack_unknown`。当前底层在无 walking lease 的 Grounded/Transitioning 状态处理 stop 时，可能明确返回 `lease_missing`。调用者保留原始原因并读取身体状态。

```rust,no_run
use doso_sdk::{BodyKind, BodyTransition, DosoClient};

async fn change_posture(
    robot: &DosoClient,
    stand: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let motion = robot.motion();
    let receipt = if stand {
        motion.stand_up(None).await?
    } else {
        motion.sit_down(None).await?
    };
    if receipt.accepted != Some(true) {
        return Err(format!("posture rejected: {}", receipt.reason).into());
    }
    let target = if stand { BodyKind::Standing } else { BodyKind::Grounded };
    let mut states = motion.watch_body_state().await?;
    while let Some(state) = states.message().await? {
        println!("{state:?}");
        if state.kind == target as i32 && state.transition == BodyTransition::None as i32 {
            return Ok(());
        }
    }
    Err("body state stream ended before the target state".into())
}
```

示例等待目标身体状态。应用可用 `tokio::time::timeout` 限定本地等待，超时后的动作结果继续通过状态核查。

源码见[姿态请求](../../src/motion/posture.rs)。

## 电机与策略

### enable_motors

```text
pub async fn enable_motors(&self, request_id: Option<&str>) -> Result<CommandReceipt>
```

请求使能电机，经过 native 仲裁与 driver 到达 Brainstem。调用参数沿用共同约定，接纳后独立读取 `control_status().motor_output_enabled` 核查软件输出门。身体姿态由 `body_state()` 查询，需要站起时另行提交 `stand_up()`。

DOSO硬件需允许电机使能，关节报告与状态满足底层条件。校准、Recovery占用、关键故障、未解决的profile故障或并发使能会影响接纳，实际拒绝原因由执行方提供。存在walking lease的底层路径先确认停止和释放，再提交使能。

### disable_motors

```text
pub async fn disable_motors(&self, request_id: Option<&str>) -> Result<CommandReceipt>
```

请求撤销底层控制lease并关闭电机输出。Brainstem可在站立或行走状态接收失能，具体请求仍经过native与driver权限入口。调用自身不提交坐下动作，应用需要有序坐下时先调用 `sit_down()` 并读取到Grounded，再提交失能。

电机回执描述执行端命令结果，`motor_output_enabled` 描述软件输出门。逐电机实际状态继续由电机诊断查询核查，软件门与命令ACK各自不能单独证明全部电机的物理输出状态。没有walking lease也可提交失能。软件急停活动时允许失能请求通过Gateway运动守卫，Gateway租约、native操作者权限及Brainstem其他物理owner的限制继续有效。正常任务执行中需先取消或停止任务，急停或控制循环恢复hold下可提交失能。

电机ACK未知时返回 `UNAVAILABLE`，机器原因为 `enable_motors_ack_unknown` 或 `disable_motors_ack_unknown`。保留原请求ID，并通过状态查询核查实际结果。源码见[电机操作](../../src/motion/motors.rs)。

```rust,no_run
use doso_sdk::DosoClient;

async fn enable_output(robot: &DosoClient) -> Result<(), Box<dyn std::error::Error>> {
    let motion = robot.motion();
    let receipt = motion.enable_motors(None).await?;
    println!("accepted={:?} reason={}", receipt.accepted, receipt.reason);
    println!("enabled={}", motion.control_status().await?.motor_output_enabled);
    Ok(())
}

async fn disable_output(robot: &DosoClient) -> Result<(), Box<dyn std::error::Error>> {
    let motion = robot.motion();
    let receipt = motion.disable_motors(None).await?;
    println!("accepted={:?} reason={}", receipt.accepted, receipt.reason);
    println!("enabled={}", motion.control_status().await?.motor_output_enabled);
    Ok(())
}
```

### profile

```text
pub async fn profile(&self) -> Result<rpc::ProfileInfo>
```

只读查询Brainstem当前locomotion profile与可用列表。查询复用现有driver端点和凭据，不申请控制权、不提交动作。未配置profile能力时可返回 `UNIMPLEMENTED`，源服务不可用时沿用查询错误。

| `ProfileInfo` 字段 | 含义 |
| --- | --- |
| `current` | 当前profile名称 |
| `available` | 执行端当前配置的profile名称列表 |
| `descriptions` | 与available按索引对应的说明 |
| `current_description` | 当前profile说明 |

名称和说明从源端透传，SDK不维护平行的型号策略清单。Profile属于运动模型和参数，运行产品模式由ProductControl/RunPlan管理，Recovery有独立会话生命周期。

### switch_profile

```text
pub async fn switch_profile(&self, name: &str, request_id: Option<&str>)
    -> Result<CommandReceipt>
```

请求切换到指定profile。`name`为执行端实际安装的非空名称，先通过 `profile()` 获取当前可用列表。DOSO硬件要求Grounded、电机输出关闭、操作者权限与Brainstem控制lease均已释放，没有校准、使能、关键故障或Recovery占用。

该公共操作只提交profile切换，保持调用者已准备好的姿态和电机状态。坐下、失能、恢复使能及站起均由调用者显式提交。内部自动profile流程有独立的driver状态机负责其阶段推进，仅在入口电机明确已使能且本次流程关闭电机时恢复输出。入口已关闭时保持关闭，未知状态继续等待有效读回。

后端另一次profile切换尚未结束时会明确拒绝，模型加载则需等待源端完成。成功ACK后，通过独立 `profile()` 再读 `current` 确认。超时或ACK未知时返回 `UNAVAILABLE`，机器原因为 `switch_profile_ack_unknown`。保留原请求ID，先查询当前profile再决定下一步。

```rust,no_run
use doso_sdk::DosoClient;

async fn switch_prepared_profile(
    robot: &DosoClient,
    installed_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let motion = robot.motion();
    println!("before={:?}", motion.profile().await?);
    let receipt = motion.switch_profile(installed_name, None).await?;
    println!("accepted={:?} reason={}", receipt.accepted, receipt.reason);
    println!("after={:?}", motion.profile().await?);
    Ok(())
}
```

示例运行前需已满足Grounded、电机关闭和控制权释放条件。以上接口的本地合同验证与设备现场验收分别记录。源码见[策略操作](../../src/motion/profile.rs)，Gateway复用共享body事务。

## 停止与急停复位

### stop

```text
pub async fn stop(&self, request_id: Option<&str>) -> Result<CommandReceipt>
```

请求普通运动停止，通过 `Motion.StopMotion` 到达 native stop 边界。返回命令回执，常见确认阶段为 `native_acknowledged`。调用需要机器人端停止能力可达，原生停止屏障未确认时返回失败或未知结果。

### emergency_stop

```text
pub async fn emergency_stop(&self, request_id: Option<&str>) -> Result<CommandReceipt>
```

请求软件急停锁存，通过 `Motion.Stop` 到达 native estop 边界。参数使用共同约定，返回命令回执。后续运动受急停锁存约束，网络或 native 回执失败通过错误返回。

### clear_emergency_stop

```text
pub async fn clear_emergency_stop(&self, request_id: Option<&str>) -> Result<CommandReceipt>
```

请求清除软件急停锁存，通过 `Motion.ClearEmergencyStop` 到达 native reset 边界。成功后由机器人端当前模式与运动 guard 决定下一次操作的准入，清除操作自身不提交速度。前置条件与复位失败原因由 native owner 返回。

```rust,no_run
use doso_sdk::DosoClient;

async fn request_stop(robot: &DosoClient) -> Result<(), Box<dyn std::error::Error>> {
    let receipt = robot.motion().stop(None).await?;
    println!("accepted={:?}, reason={}", receipt.accepted, receipt.reason);
    Ok(())
}

async fn request_estop(robot: &DosoClient) -> Result<(), Box<dyn std::error::Error>> {
    let latched = robot.motion().emergency_stop(None).await?;
    if latched.accepted != Some(true) {
        return Err(format!("emergency stop unconfirmed: {}", latched.reason).into());
    }
    Ok(())
}

async fn reset_estop(robot: &DosoClient) -> Result<(), Box<dyn std::error::Error>> {
    let reset = robot.motion().clear_emergency_stop(None).await?;
    println!("reset accepted={:?}, reason={}", reset.accepted, reset.reason);
    Ok(())
}
```

源码见[停止操作](../../src/motion/stop.rs)。

### resume

```text
pub async fn resume(&self, request_id: Option<&str>) -> Result<CommandReceipt>
```

请求释放当前模式的 native 运动 hold，经共享命令 journal 和 native 恢复操作返回回执。HTTP 与 gRPC 使用同一业务实现，重复请求沿用现有 journal。参数采用共同约定，当前确认阶段为 `native_acknowledged`。

恢复检查当前控制权、软件急停、输入就绪与控制循环恢复条件。拒绝原因由 native owner 提供。恢复沿用当前 Product 的运行模式，之后由调用者提交新目标或新速度。

恢复收到明确拒绝ACK时保留 native 原因。ACK缺失、超时或身份不匹配时返回 `UNAVAILABLE`，机器原因为 `motion_resume_ack_unknown`，调用者保留原请求ID并核查状态。此时提交结果保持未知。

`details` 保留 `native_reason`、`native_request_id`、`resume_was_required`、`goal_reissue_required`、`fresh_operator_command_required` 等恢复信息，缺少依据时相关字段保持未知。native ACK 中的原因优先于命令前状态快照，`previous_motion_restored` 为 false。

```rust,no_run
use doso_sdk::DosoClient;

async fn resume_motion(robot: &DosoClient) -> Result<(), Box<dyn std::error::Error>> {
    let receipt = robot.motion().resume(None).await?;
    println!("accepted={:?} reason={}", receipt.accepted, receipt.reason);
    println!("details={:?}", receipt.details);
    Ok(())
}
```

源码见[恢复操作](../../src/motion/resume.rs)。

## 身体与控制状态

### body_state

```text
pub async fn body_state(&self) -> Result<rpc::BodyState>
```

读取 Brainstem 当前身体有限状态机，无参数。需要机器人端身体状态源可达，源不可用时返回 `UNAVAILABLE`，旧服务缺少方法时返回 `UNIMPLEMENTED`。

### watch_body_state

```text
pub async fn watch_body_state(&self) -> Result<tonic::Streaming<rpc::BodyState>>
```

订阅当前身体状态和后续源事件，无参数。该流跟随源事件，推送频率随实际状态变化。建立失败按查询错误处理，流中断通过 `tonic::Status` 返回。

| `BodyState` 字段 | 含义 |
| --- | --- |
| `kind` | 原始 `i32`，已知 `BodyKind` 包括 Zero、Grounded、Standing、Walking、Transitioning |
| `transition` | 原始 `i32`，已知 `BodyTransition` 包括 None、StandUp、SitDown、Gesture |
| `gesture_name` | 源端动作名称，可为空 |

未知枚举整数原样保留，使用 `BodyKind::try_from()` 等转换识别。当前消息没有采样时间字段。

### control_status

```text
pub async fn control_status(&self) -> Result<rpc::BodyControlStatus>
```

只读查询 Brainstem 的控制状态，无参数。需要控制状态源可达，查询不申请或续约硬件控制 lease。

| 字段 | 含义与单位 |
| --- | --- |
| `body` | 可选身体状态 |
| `motor_output_enabled`、`critical_motor_fault` | 电机输出门与关键故障 |
| `source_owner`、`source_owner_id` | 源端 owner，已知枚举 None、Grpc、Yunzhuo，原始整数保留 |
| `grpc_lease_active`、`lease_remaining_ms` | 源端 gRPC lease 是否活动，以及剩余毫秒 |
| `last_accepted_sequence`、`source_last_decision` | 源端已接纳序号与 Brainstem 决策原始整数 |
| `source_ready_for_walk` | 源端是否就绪 |
| `accepted_count`、`rejected_count` | 源端命令接纳与拒绝计数 |

GRPC owner 可对应 `lingtu-driver@robot`。SDK 操作者的 native authority 在遥操作会话内管理。

### control_state

```text
pub async fn control_state(&self) -> Result<rpc::ControlState>
```

只读查询 native 操作者权限快照。查询自身不申请或续约控制权。全局查询缺少具体 Teleop 会话身份，`held_by_client` 保持 `None`。会话持权通过 [会话 control_state](#会话-control_state) 读取。

### watch_control_state

```text
pub async fn watch_control_state(&self) -> Result<tonic::Streaming<rpc::ControlState>>
```

按目标最高 50 Hz 订阅同源全局快照。该接口提供周期快照，调用者从最新值判断当前状态。中间状态可能合并，逐次失权、可申请变化的完整事件历史需要另行记录。

| `ControlState` 字段 | 含义 |
| --- | --- |
| `available` | optional，native 当前是否没有活动操作者权限，具体申请仍由当时仲裁决定 |
| `held_by_client` | optional，仅关联到具体 Teleop 会话时表达该会话是否持权 |
| `owner`、`owner_epoch` | 当前 owner 与权限世代 |
| `holding` | optional，native 是否处于 hold |
| `reason` | native 状态或不可用原因 |
| `received_age_ms`、`fresh` | native DDS 接收年龄与新鲜度，会话本地读取还累计 SDK 接收后的等待时间。网络传输时间未知 |
| `time` | 可选源时间，未知时保持缺失 |

源状态过期时 `fresh=false`，`available` 与 `held_by_client` 都为 `None`，防止使用旧状态推断当前控制权。连接中断通过流错误返回，读取成功仍需检查新鲜度。

新鲜度包含 native 接收年龄与 SDK 本地缓存年龄，跨网络传输的排队时长未知。它描述最后观测的权限快照，实际指令仍由 native owner 在执行时仲裁。

```rust,no_run
use doso_sdk::{BodyKind, DosoClient};

async fn inspect(robot: &DosoClient) -> Result<(), Box<dyn std::error::Error>> {
    let motion = robot.motion();
    let body = motion.body_state().await?;
    println!("body={:?}", BodyKind::try_from(body.kind));
    let control = motion.control_status().await?;
    println!("owner={} lease_ms={}", control.source_owner_id, control.lease_remaining_ms);
    Ok(())
}
```

订阅示例见[站起与坐下](#stand_up)。源码见[身体状态](../../src/motion/body.rs)和[控制状态](../../src/motion/control.rs)。

## 运动诊断快照

### status

```text
pub async fn status(&self) -> Result<rpc::Snapshot>
```

读取 native 导航与运动控制的 Gateway 投影，无参数。返回 `Snapshot.fields` 中的可扩展诊断，包括模式、状态与 gate 原因，具体字段由状态 owner 提供。字段缺失时保持未知。

### watch_state

```text
pub async fn watch_state(&self) -> Result<tonic::Streaming<rpc::Snapshot>>
```

持续订阅与 `status()` 同源的诊断快照，无参数，当前 Gateway 目标最高 100 Hz。重复快照表达当前缓存状态。

### watch_state_at

```text
pub async fn watch_state_at(&self, frequency_hz: u32)
    -> Result<tonic::Streaming<rpc::Snapshot>>
```

按指定目标最高频率订阅诊断快照。`frequency_hz` 为整数 Hz，支持 1–100，超出范围由服务端返回 `INVALID_ARGUMENT`。状态生成耗时、网络和运行负载会影响实际频率。

上述方法要求运行中的 Gateway 状态服务。订阅占用共享名额，耗尽时返回 `RESOURCE_EXHAUSTED`。该频率参数控制快照推送，设备传感器采样率由对应数据源配置。

```rust,no_run
use doso_sdk::DosoClient;

async fn read_status(robot: &DosoClient) -> Result<(), Box<dyn std::error::Error>> {
    println!("{:?}", robot.motion().status().await?.fields);
    let mut states = robot.motion().watch_state_at(50).await?;
    while let Some(state) = states.message().await? {
        println!("{:?}", state.fields);
    }
    Ok(())
}
```

采用默认目标频率时，将示例中的 `watch_state_at(50)` 改为 `watch_state()`。源码见[状态查询](../../src/motion/status.rs)和[状态订阅](../../src/motion/watch.rs)。

## 定位运动反馈

这些接口读取 Gateway 已接入的定位里程计输入，当前 `source="localization"`。real 原生路径由 SLAM 估计提供，生产者须明确标记完整六轴反馈可用，默认构造的Twist保持未知。

需要明确可用的完整六轴反馈、正确坐标系和新鲜缓存。缺数据、字段不完整、定位失效或缓存超过当前 2 秒阈值时返回 `UNAVAILABLE`，订阅以相应流错误结束。

### velocity

```text
pub async fn velocity(&self) -> Result<rpc::VelocitySample>
```

无参数，读取当前定位估计速度。SDK 检查线速度、角速度消息存在且数值有限，frame/source 非空，接收年龄有效，`fresh` 没有明确为 false。损坏回复返回 `ErrorKind::Protocol`。

### watch_velocity

```text
pub async fn watch_velocity(&self) -> Result<tonic::Streaming<rpc::VelocitySample>>
```

无参数，以目标最高 100 Hz 订阅速度缓存快照。真实 real 默认定位状态导出约 10 Hz，因此新源样本默认约 10 Hz，重复缓存快照可能具有相同速度。每个流内样本由 Gateway 校验，SDK 返回原生 tonic 流。

| `VelocitySample` 字段 | 含义与单位 |
| --- | --- |
| `linear_m_s` | 三轴线速度，m/s，当前表达于 body |
| `angular_rad_s` | 三轴角速度，rad/s，当前表达于 body |
| `frame_id` | 速度表达坐标系，当前为 `body` |
| `received_age_ms` | 可选 Gateway 接收后缓存年龄，ms |
| `fresh` | 可选新鲜度，当前可用回复为 true |
| `source` | 当前为 `localization` |
| `time` | 可选 `SampleTime`，当前缓存缺少明确源时钟域，保持 `None` |

### odometry

```text
pub async fn odometry(&self) -> Result<rpc::OdometrySample>
```

无参数，读取完整定位位姿与六轴速度。SDK 校验向量存在且有限、四元数平方范数与 1 的差不超过 `1e-3`、parent/child 非空且不同，以及 source/freshness。损坏回复返回 `ErrorKind::Protocol`。

### watch_odometry

```text
pub async fn watch_odometry(&self) -> Result<tonic::Streaming<rpc::OdometrySample>>
```

无参数，按目标 10 Hz、每 100 ms 订阅当前里程计快照。源更新率与传输负载决定实际新样本频率，建立及流内错误使用共同订阅约定。

| `OdometrySample` 字段 | 含义与单位 |
| --- | --- |
| `position_m` | child 原点在 parent 中的位置，m |
| `orientation` | 单位四元数 x/y/z/w，将 child 向量旋转到 parent |
| `linear_velocity_m_s` | child 中的三轴线速度，m/s |
| `angular_velocity_rad_s` | child 中的三轴角速度，rad/s |
| `parent_frame`、`child_frame` | 当前为 `map`、`body` |
| `received_age_ms`、`fresh`、`source`、`time` | 与速度反馈采用相同语义 |

`received_age_ms` 衡量 Gateway 接收后的年龄。设备采样时间与接收年龄各自独立，当前未知源时间保持缺失。

```rust,no_run
use doso_sdk::DosoClient;

async fn feedback(robot: &DosoClient) -> Result<(), Box<dyn std::error::Error>> {
    println!("{:?}", robot.motion().velocity().await?);
    println!("{:?}", robot.motion().odometry().await?);
    let mut velocities = robot.motion().watch_velocity().await?;
    if let Some(sample) = velocities.message().await? {
        println!("linear={:?}, angular={:?}", sample.linear_m_s, sample.angular_rad_s);
    }
    drop(velocities);
    let mut odometry = robot.motion().watch_odometry().await?;
    while let Some(sample) = odometry.message().await? {
        println!("pose={:?}, orientation={:?}", sample.position_m, sample.orientation);
    }
    Ok(())
}
```

源码见[速度](../../src/motion/velocity.rs)和[里程计](../../src/motion/odometry.rs)。

## 受控遥操作

### teleop

```text
pub async fn teleop(&self) -> Result<Teleop<'a>>
```

该方法属于 `Motion<'a>`，打开双向 gRPC 连接并等待初始输入窗口。`Teleop<'a>` 借用同一个 `DosoClient`，会话内部维护输入序号、后台回执接收与有界发送队列。

需要支持连续输入合同的 Gateway 和可用的共享操作者连接名额。Web 与 SDK 使用同一名额，已被占用或共享流名额耗尽时返回 `RESOURCE_EXHAUSTED`。旧服务缺少 RPC 时返回 `UNIMPLEMENTED`，开流回复不符合合同则返回 `ErrorKind::Protocol`。

机器人每 50 ms 签发新窗口，每个窗口固定有效 350 ms，最多保留八个窗口。同窗支持多帧输入。SDK 在显式发布时绑定窗口与递增序号，排队后保留原始窗口期限。首条有效速度输入触发 native authority 申请，底层 owner 与运动 guard 决定能否接受。

机器人连续 3 秒无客户端输入时关闭会话。服务端主动窗口推送不延长该期限。

### claim_control

```text
pub async fn claim_control(&self, request_id: Option<&str>) -> Result<Teleop<'a>>
```

该方法属于 `Motion<'a>`，打开同一套 Teleop 双向流并显式申请 native authority，等待接纳后返回会话句柄。申请只提交权限操作。拒绝返回带请求身份与原因的 SDK 错误，会话建立沿用 `teleop()` 的资源限制。

### claim

```text
pub async fn claim(&mut self, request_id: Option<&str>) -> Result<rpc::TeleopReply>
```

在已打开的 Teleop 会话中单次申请或重新申请 native authority，返回该请求的回执。调用者检查 `accepted` 和 `reason`。native lease 当前为 1 秒，SDK 没有后台保活。会话仍存在时，权限到期后可显式调用 `claim()` 重新申请，接纳由当时的 native 仲裁决定。

缺少 native 申请 ACK 时返回 `UNAVAILABLE` 与 `control_claim_ack_unknown`，SDK 保留请求 ID 和未知提交状态。明确拒绝回执保留拒绝原因，`claim_control()` 将其作为 `FAILED_PRECONDITION` 返回。

`teleop()` 的首条有效速度输入继续支持延迟申请。窗口推送和状态读取都不续约 native lease。应用按当下输入提交速度，静止期间如需保留权限，应显式管理申请及其回执。

### 会话 control_state

```text
pub fn control_state(&self) -> Result<Option<rpc::ControlState>>
```

该方法属于 `Teleop`，读取后台最近收到的会话权限快照。尚无快照时返回 `None`，后台流失败返回 SDK 错误。`held_by_client` 依据当前会话与 native owner 对照设置，`owner` 使用同一份 native 状态。快照目标最高 50 Hz，可合并中间值，读取本地缓存自身不续约控制权。

```rust,no_run
use doso_sdk::DosoClient;

async fn inspect_control(robot: &DosoClient) -> Result<(), Box<dyn std::error::Error>> {
    let motion = robot.motion();
    println!("global={:?}", motion.control_state().await?);
    let mut session = motion.claim_control(None).await?;
    println!("session={:?}", session.control_state()?);
    let renewed = session.claim(None).await?;
    println!("accepted={} reason={}", renewed.accepted, renewed.reason);
    let released = session.release_control(None).await?;
    println!("released={} reason={}", released.accepted, released.reason);
    Ok(())
}
```

### publish_velocity

```text
pub fn publish_velocity(
    &mut self,
    velocity: rpc::TeleopVelocity,
    request_id: Option<&str>,
) -> Result<String>
```

将当下新输入提交至 SDK 发送队列，立即返回本地入队的请求 ID。机器人接纳通过 [latest_reply](#latest_reply) 观察。适合调用者按 50 Hz 产生新输入。

| `TeleopVelocity` 字段 | 单位、范围与行为 |
| --- | --- |
| `vx_m_s` | body X 方向线速度，m/s，正向为前，必须有限 |
| `vy_m_s` | body Y 方向线速度，m/s，正向为左，必须有限 |
| `yaw_rad_s` | 绕 body +Z 的角速度，rad/s，必须有限 |
| `deadman` | 持续运动时为 true，false 请求 hold |
| `manual_mode` | 默认 false，true 需要机器人 `teleop_avoid` 控制模式允许 |

数值限幅和可接受运动范围由机器人当前配置与 native 控制端决定。SDK 校验有限数值，没有统一的固定速度区间。

发送队列最多一个待发位置，满时返回 `RESOURCE_EXHAUSTED`，本次速度没有入队。下一次重新采集并提交当前输入。SDK 没有后台自动重发旧速度的行为，机器人入口与 native publisher 对积压速度保留最新值。

### send_velocity

```text
pub async fn send_velocity(
    &mut self,
    velocity: rpc::TeleopVelocity,
    request_id: Option<&str>,
) -> Result<rpc::TeleopReply>
```

参数与 `publish_velocity()` 相同，发送一次速度并等待该请求的回执，沿用已建立的窗口。返回 `TeleopReply`，调用者检查 `accepted`、`ingress_accepted` 和 `reason`。输入无效、队列满、源端拒绝或连接故障按对应结果处理。

等待超时返回 `ErrorKind::Timeout`，请求身份及未知提交语义保留。取消这个等待会关闭会话，触发机器人端释放流程。

### latest_reply

```text
pub fn latest_reply(&self) -> Result<Option<rpc::TeleopReply>>
```

读取后台接收的最新命令回执，无参数。尚无命令回执时返回 `None`，较慢的调用者可能跳过中间回执。通过 `request_id`、`sequence` 关联实际请求，后台流失败返回 SDK `Error`。

| `TeleopReply` 字段 | 含义 |
| --- | --- |
| `request_id`、`sequence` | 请求身份与会话内输入序号 |
| `accepted` | 当前请求是否接纳 |
| `reason` | 原因，例如 `queued`、`held`、`input_expired`、`stale_sequence`、`safety_stop` |
| `ingress_accepted` | 速度已进入 Gateway 可替换队列 |
| `final_output_confirmed` | hold/release 的 native 最终输出确认 |
| `input_window` | 机器人生成的窗口编号，由 SDK 内部维护 |
| `control` | 可选会话权限快照，由后台接收并供 `Teleop::control_state()` 读取 |

主动窗口更新由 SDK 后台消费。`latest_reply()` 保留最近的命令回执。

### hold

```text
pub async fn hold(&mut self, request_id: Option<&str>) -> Result<rpc::TeleopReply>
```

请求停止当前会话的运动，等待 native hold 回执。检查 `accepted`、`final_output_confirmed` 和 `reason`，无活动命令时可返回 `already_idle` 且没有最终输出确认。

机器人作废旧窗口，清理 native 待发速度，并在回执中提供新窗口。会话继续存在，后续显式新输入使用新窗口。停止屏障失败会返回拒绝或 RPC 错误，超时保留未知结果。

### close

```text
pub async fn close(self, request_id: Option<&str>) -> Result<rpc::TeleopReply>
```

消耗当前会话，执行 hold/release 并等待回执，释放操作者连接名额。无活动 native authority 时可返回 `already_idle`。底层 lease 或停止确认失败时检查真实原因。

丢弃 `Teleop` 会取消流并触发机器人端清理。需要获得释放回执的调用使用显式 `close()`。关闭、超时与取消均保留当前平台停止合同的限制，物理停稳通过设备状态确认。

### release_control

```text
pub async fn release_control(mut self, request_id: Option<&str>) -> Result<rpc::TeleopReply>
```

消耗会话句柄，与 `close()` 共用 hold、native authority 释放与连接关闭流程。返回同一类回执，调用者检查 `accepted`、`final_output_confirmed` 和 `reason`。新建会话后可重新申请控制权。

### 50 Hz 新输入示例

```rust,no_run
use doso_sdk::{Code, DosoClient, TeleopVelocity};
use std::time::Duration;

async fn drive(
    robot: &DosoClient,
    mut read_input: impl FnMut() -> Option<TeleopVelocity>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut session = robot.motion().teleop().await?;
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let Some(velocity) = read_input() else { break };
        if !velocity.deadman { break; }
        match session.publish_velocity(velocity, None) {
            Ok(id) => println!("queued locally: {id}"),
            Err(error) if error.code() == Code::ResourceExhausted => continue,
            Err(error) => return Err(error.into()),
        }
        if let Some(reply) = session.latest_reply()? {
            println!("ack={} accepted={} reason={}", reply.request_id, reply.accepted, reply.reason);
            if !reply.accepted { break; }
        }
    }
    let released = session.close(None).await?;
    if !released.accepted {
        return Err(format!("release unconfirmed: {}", released.reason).into());
    }
    Ok(())
}
```

每次 tick 从调用者取得当下输入。队列满时跳过本次样本，下个 tick 再读取新输入。本地 loopback 的 SDK→Gateway→native publisher 链路已验证约 50 Hz，包含回程延迟注入，末端 native owner 使用测试替身。设备端无线链路与实际运动频率仍需实测。具体数值和测试边界见[本地验证记录](../grpc-v1-progress.md)。

### 单次等待与 hold 示例

```rust,no_run
use doso_sdk::{DosoClient, TeleopVelocity};

async fn send_then_hold(
    robot: &DosoClient,
    current_input: TeleopVelocity,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut session = robot.motion().teleop().await?;
    let reply = session.send_velocity(current_input, None).await?;
    println!("ingress={} reason={}", reply.ingress_accepted, reply.reason);
    let held = session.hold(None).await?;
    println!("held={} final_output={}", held.accepted, held.final_output_confirmed);
    let released = session.close(None).await?;
    if !released.accepted {
        return Err(format!("release unconfirmed: {}", released.reason).into());
    }
    Ok(())
}
```

完整实现见[遥操作会话](../../src/motion/teleop.rs)，协议见[Motion 服务](../../proto/motion.proto)和[运动消息](../../proto/messages/motion.proto)。

## 自定义控制器

客户自行选择策略语言、模型格式、推理框架及程序部署方式。SDK规范机器人控制接入，持续接收客户生成的逐关节目标。客户程序使用所需的IMU、关节等观测自行计算目标，实际电机通信与控制权限由native driver和Brainstem管理。

### 接口与代码对应

以下方法实现在 [external.rs](../../src/motion/external.rs)。RPC定义在 [proto/motion.proto](../../proto/motion.proto)，消息字段定义在 [proto/messages/controller.proto](../../proto/messages/controller.proto)。`ExternalControl`会话方法共用一条双向流，后台接收回执及状态，`state()`和`latest_reply()`读取本地缓存。

| Rust接口 | 公共RPC或流内动作 | 机器人端处理 |
| --- | --- | --- |
| `Motion::controller()` | `Motion.Controller(Empty)` | 读取native控制器状态快照 |
| `Motion::external_control()` | 建立 `Motion.ExternalControl` 流，首先发送 `enter` | native仲裁进入外控，driver调用Brainstem `Enter`，暂停内置行为 |
| `ExternalControl::publish_joints()` | 同一流的 `frame`，携带整帧目标、序号和原输入窗口 | native client通过DDS发给driver，再进入Brainstem `Control`及电机入口 |
| `ExternalControl::enable()` | 同一流的 `enable` | native仲裁后由driver调用Brainstem `Enable` |
| `ExternalControl::disable()` | 同一流的 `disable` | native仲裁后由driver调用Brainstem `Disable`，保留外控模式 |
| `ExternalControl::close()` | 同一流的 `close` | native仲裁后由driver调用Brainstem `Exit`，关闭输出并交还内置控制 |
| `ExternalControl::state()` | 读取后台维护的 `ControllerState` 缓存 | 状态来源为driver上报的Brainstem状态 |
| `ExternalControl::latest_reply()` | 读取最新带请求身份的回执缓存 | 保留队列接纳与执行端帧验证的区别 |
| SDK自动心跳 | 同一流的 `heartbeat`，序号为0 | driver调用Brainstem `Heartbeat`维持会话 |

以上接口已具备Rust、Gateway、native、driver及Brainstem实现。各文件与运行位置见[架构中的代码对应](../architecture.md#外部运控的代码对应)，本地测试与实机未验证范围见[验证记录](../grpc-v1-progress.md#外部运控模式2026-11-07)。

### controller

```text
pub async fn controller(&self) -> Result<rpc::ControllerState>
```

只读查询当前builtin/external模式及执行端状态。源缺失或过期时模式为Unknown，`fresh=false`、`active=false`，关节数量缺失时保持0。查询只读取状态。

| `ControllerState` 字段 | 含义 |
| --- | --- |
| `mode` | Unknown、Builtin或External，区别内置与外部控制 |
| `session_id` | 当前外部会话身份 |
| `active` | 当前外部会话是否仍有有效权限，电机输出另看motor_output_enabled |
| `motor_output_enabled` | 软件输出门状态 |
| `last_sequence` | 执行端已接纳的最新关节帧序号，电机关闭时可表示已准备目标 |
| `reason` | 执行端当前状态与原因 |
| `received_age_ms`、`fresh` | 可选接收年龄与新鲜度，未知年龄保持缺失 |
| `motor_disable_confirmed` | 执行端是否提供电机关闭确认，缺少确认时为false |
| `joint_count` | 执行端声明的一帧关节数量，当前DOSO路径为16，未知为0 |

`last_sequence`表达机器人端帧处理，逐电机物理结果继续从设备反馈核查。网络传输耗时不包含在单纯的接收缓存年龄中。

### external_control

```text
pub async fn external_control(&self, request_id: Option<&str>)
    -> Result<ExternalControl<'a>>
```

该方法属于 `Motion<'a>`，开启 `Motion.ExternalControl` 双向流，以enter请求ID标识会话。进入要求Grounded、电机关闭及现有控制权已释放，native执行端暂停内置行为及其输出，并保留传感器与电机通信。SDK等待enter接纳与首个有效输入窗口后返回句柄，电机随后由显式enable管理。

外部会话与其他长期订阅共享8个Gateway流名额，名额耗尽返回 `RESOURCE_EXHAUSTED`。API key与TLS沿用客户端配置。原生动作ACK未知时返回 `UNAVAILABLE`，机器原因采用 `external_enter_ack_unknown`、`external_enable_ack_unknown`、`external_disable_ack_unknown` 或 `external_close_ack_unknown`，调用者保留原请求ID并核查控制器状态。

### 外部会话方法

下列方法属于 `ExternalControl`。

```text
pub fn state(&self) -> Result<Option<rpc::ControllerState>>
pub fn latest_reply(&self) -> Result<Option<rpc::ExternalControlReply>>
pub fn publish_joints(&mut self, joints: Vec<rpc::JointCommand>, request_id: Option<&str>)
    -> Result<String>
pub async fn enable(&mut self, request_id: Option<&str>) -> Result<rpc::ExternalControlReply>
pub async fn disable(&mut self, request_id: Option<&str>) -> Result<rpc::ExternalControlReply>
pub async fn close(self, request_id: Option<&str>) -> Result<rpc::ExternalControlReply>
```

`state()`和`latest_reply()`读取后台缓存，较慢的调用者可能跳过中间值。周期状态更新目标为50Hz，命令回执也携带当前状态；该频率不表示每帧硬件采样率。缓存过期时`active=false`。`publish_joints()`立即返回本地入队ID，队列满时返回 `RESOURCE_EXHAUSTED`，应用在下一次采样提交新目标。

一帧包含全部 `joint_count` 个关节，顺序使用机器人关节顺序。每个 `JointCommand` 提供 `q` 弧度、`dq` 弧度/秒、`kp`、`kd`、`tau` 牛·米，数值必须有限，kp/kd非负。实际执行边界还检查目标与硬件条件。

当前DOSO的16项沿用Brainstem `JointsMatrix`，与关节反馈ID一致：

| 索引 | 关节顺序 |
| --- | --- |
| 0–2 | 右前：hip、thigh、calf |
| 3–5 | 左前：hip、thigh、calf |
| 6–8 | 右后：hip、thigh、calf |
| 9–11 | 左后：hip、thigh、calf |
| 12–15 | 右前、左前、右后、左后 foot |

`q/dq/tau`直接使用现有电机协议坐标，SDK不替客户转换零位、方向或减速比。模型输出需先对齐目标机器人的标定；执行端编码范围检查不等同于机械关节限位。

帧回复的 `accepted/ingress_accepted` 表达native队列接纳，`state.last_sequence`提供后续执行端接纳序号。SDK保存原始窗口，Gateway和driver逐级原样转交。后台窗口更新、队列处理和发送均保持原帧窗口身份。机器人每20ms签发窗口，每个窗口有效100ms，过期帧由执行端拒绝。

SDK每100ms发送一次heartbeat，序号固定0，回复作为无请求ID的状态更新。Heartbeat只续约300ms会话权限，输出有独立的100ms新帧期限。enter、frame、enable、disable和close的非零序号严格递增。持续运动要求调用方持续产生新目标。

进入后先提交一帧当前准备好的保持目标，经 `last_sequence` 确认，再请求 `enable()`。使能后的新目标持续替换当前输出。`disable()`关闭输出并保留外部模式，之后能否再次使能由当前会话和目标新鲜度决定。

`close()`消耗句柄，显式关闭电机输出并交还builtin idle。丢弃句柄、半关闭、断线或输入超时仅关闭外部输出，保留External隔离。客户显式交还控制器后，内置行为才恢复，电机继续保持关闭。

```rust,no_run
use doso_sdk::{Code, DosoClient, JointCommand};
use std::time::Duration;

async fn run_external(
    robot: &DosoClient,
    initial_targets: Vec<JointCommand>,
    mut next_targets: impl FnMut() -> Option<Vec<JointCommand>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut session = robot.motion().external_control(None).await?;
    let initial_id = session.publish_joints(initial_targets, None)?;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(reply) = session.latest_reply()? {
                if reply.request_id == initial_id {
                    if !reply.ingress_accepted {
                        return Err(format!("initial frame rejected: {}", reply.reason).into());
                    }
                    if session.state()?.is_some_and(|state| state.fresh && state.last_sequence >= reply.sequence) {
                        return Ok::<(), Box<dyn std::error::Error>>(());
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await??;
    let enabled = session.enable(None).await?;
    if !enabled.accepted {
        return Err(format!("external enable rejected: {}", enabled.reason).into());
    }
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let Some(targets) = next_targets() else { break };
        match session.publish_joints(targets, None) {
            Ok(_) => {}
            Err(error) if error.code() == Code::ResourceExhausted => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let closed = session.close(None).await?;
    if !closed.accepted {
        return Err(format!("external handover unconfirmed: {}", closed.reason).into());
    }
    Ok(())
}
```

调用前由客户完成策略初始化，`initial_targets`和`next_targets`来自其真实控制程序。进入前的站坐与电机准备沿用已有接口。示例中的20ms节拍表达调用方式，目标设备性能与控制效果仍需现场验收。

源码见[外部会话](../../src/motion/external.rs)，消息见[控制器协议](../../proto/messages/controller.proto)。
