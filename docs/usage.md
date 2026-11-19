# DOSO Rust SDK

`doso-sdk` 是 V1 的异步 Rust 客户端。运动和实时观测优先通过 `BrainstemClient` 以 **gRPC + Protobuf** 直连机器狗 `13145`；地图、SLAM、导航和任务等高层能力由可选的 `DosoClient` 连接 LingTu Gateway。两套客户端共用错误类型，但协议和服务端职责不同。

本页提供构建与常用调用流程。完整函数签名、参数、返回和失败行为按领域维护于API手册。[API手册](api/README.md)

## 构建与接入

需要 Rust 1.94 或更新版本，以及 PATH 中可用的 `protoc`。也可以用 `PROTOC` 环境变量指定编译器路径。`build.rs` 从本 crate 的 `proto/` 和内置 `brainstem/protocol/` 生成 Rust 类型和客户端；Cargo.lock 固定仓库已解析的依赖版本。

在 DOSO-SDK 根目录执行：

```text
cargo build --workspace --locked
cargo test --workspace --locked
cargo run -p doso-sdk --example read_brainstem -- http://127.0.0.1:13145
```

在应用的 Cargo.toml 中固定 SDK 版本，并启用 Tokio 运行时：

```toml
[dependencies]
doso-sdk = { git = "https://github.com/Kitjesen/DOSO-SDK.git", tag = "v0.2.0" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

本地开发可将 SDK 依赖改为 `doso-sdk = { path = "../DOSO-SDK" }`，路径相对于应用的 Cargo.toml。库通过 Cargo 依赖集成，目前未发布到 crates.io。

```rust,no_run
use doso_sdk::{DosoClient, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    let info = robot.system().info();
    println!("firmware_version: {:?}", info.firmware_version);
    println!("robot_version: {:?}", info.robot_version);
    println!("robot_id: {:?}", info.robot_id);
    println!("{:?}", robot.system().readiness().await?);
    println!("{:?}", robot.maps().list().await?);
    println!("{:?}", robot.localization().status().await?);
    println!("{:?}", robot.observations().motors().await?);
    println!("{:?}", robot.observations().motor_voltages().await?);
    Ok(())
}
```

`http://` 使用 HTTP/2 明文 gRPC 和 Protobuf。`DosoClient::connect().await` 建立通道后完成只读 `System.Handshake`。机器狗上的本地程序可使用 `BrainstemClient::connect("http://127.0.0.1:13145")`。从电脑连接配置了 mTLS 的机器狗时，使用 `https://` 和已配对的客户端证书。[直连 API](api/brainstem.md)

`system().info()` 同步借用最近成功握手的 `SystemInfo`，包含固件版本 `firmware_version`、机器人版本 `robot_version` 和机器人 ID `robot_id`。固件版本取当前运行发布包 `metadata.json.version`，机器人版本取 RunPlan 或 `LINGTU_ROBOT` 中的型号，机器人 ID 读取设备端 `DOSO_DEVICE_ID`。缺失项返回 `None`；非空 ID 不是规范 UUID v4 时，握手返回 `FAILED_PRECONDITION` 和 `invalid_robot_id`。具体来源与统一发布规则见[机器人信息与固件发布](release.md)。

重新连接会刷新这三项信息。动态运行环境和会话查询 `session()`，健康与就绪查询 `health()` 和 `readiness()`。连接采用观察式握手，既有命令经过原有仲裁，独立的控制权申请/释放 API 仍待接入。

需要配套支持 `System.Handshake` 的 Gateway。旧服务没有此 RPC 时连接返回 `UNIMPLEMENTED`；请求的协议版本不匹配时返回 `FAILED_PRECONDITION` 和 `protocol_mismatch`。

`Config::new(endpoint)` 提供 `endpoint/client_id/api_key/timeout/tls_ca_certificate` 配置，通过 `DosoClient::with_config(config).await` 连接。普通请求默认超时 10 秒，重定位为 60 秒。认证和 TLS 的配置见下一节。

## API key 与 TLS

Gateway 的 gRPC listener 支持独立 API key 与 TLS 配置，配置在服务启动时读取。

| 机器人端环境变量 | 行为 |
| --- | --- |
| `LINGTU_GRPC_API_KEY` | 配置后，所有 RPC 要求恰好一个匹配的 `x-api-key` metadata。缺失、错误或重复凭据返回 `UNAUTHENTICATED` |
| `LINGTU_GRPC_REQUIRE_API_KEY=1` | 要求启动时配置 API key，缺失时启动失败 |
| `LINGTU_GRPC_TLS_CERT_FILE` | TLS 服务端 PEM 证书文件，与私钥一起配置 |
| `LINGTU_GRPC_TLS_KEY_FILE` | TLS PEM 私钥文件，与证书一起配置 |

未配置 key 和证书时采用开发用明文 listener。API key 与 TLS 分别启用，传输加密需要配置 TLS 并连接 `https://` 端点。文件缺失、为空或证书无效时 listener 启动失败。gRPC 的凭据与 MCP 的 `LINGTU_API_KEY` 分别管理。

```rust,no_run
use doso_sdk::{Config, DosoClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = Config::new("https://robot.example:5052");
    config.api_key = Some(std::env::var("DOSO_API_KEY")?);
    config.tls_ca_certificate = Some(std::fs::read("robot-ca.pem")?);
    let robot = DosoClient::with_config(config).await?;
    println!("{:?}", robot.system().info());
    Ok(())
}
```

HTTPS 使用系统信任根，`tls_ca_certificate` 可追加私有 CA 的 PEM 字节。服务证书须匹配连接地址中的主机名。为 HTTP 端点配置 CA 或传入空 CA 会返回输入错误。API key metadata 标记为 sensitive，并随所有七域请求发送。

## 职责与源码

```text
src/
├── lib.rs             公共导出和生成的 rpc 模块
├── client/            连接、共享 channel、请求与回执
├── common/            请求身份和通用输入检查
├── error/             gRPC 与业务错误
├── system/            系统状态和能力
├── motion/            运动模块入口、停止、状态查询和状态订阅
├── localization/      位姿、定位、重定位
├── maps/              地图资产和产品事务
├── navigation/        目标、任务和状态流
├── observations/      场景、相机状态、IMU/关节流与电机诊断
└── tasks/             指令与 Agent run
```

运动领域按停止、状态查询和订阅拆分文件，观测领域按场景、相机、IMU、关节、电机状态和电压查询拆分文件。各自的 `mod.rs` 保留模块入口与共享类型，公开数据类型从 Protobuf 生成。

七个领域的逐方法参考分别由对应API页维护，包含当前公开查询、操控和订阅方法。[system](api/system.md) · [motion](api/motion.md) · [localization](api/localization.md) · [maps](api/maps.md) · [navigation](api/navigation.md) · [observations](api/observations.md) · [tasks](api/tasks.md)

位姿、命令、导航任务、地图操作和产品操作具有显式 Protobuf 字段。健康、能力等可扩展诊断使用 `Snapshot.fields: google.protobuf.Struct`；`details` 保留服务端补充信息，因此并非所有状态字段都已固定成独立消息。

`system().readiness_state()` 读取现有 `readiness()` 并转换成明确的 `ReadinessState`：`ready`、`data_ready`、`motion_ready` 和 `non_motion_safe` 分别保留源端含义。源未提供的布尔值为 `None`，不会默认成 `false`；该查询不意味着获得运动控制权。

`system().robot_status()` 查询当前模式、就绪与原因，软件急停只在 native 状态新鲜且有明确值时返回。整机健康与硬件急停缺少可信统一来源时分别保持 `Unknown`、`None`；不把模块在线或未就绪推断为整机健康/故障。

`tasks().get_run_state(run_id)` 基于现有任务查询返回 `Option<TaskRunState>`；无 AgentPlanner 或任务已不再是当前运行时返回 `None`，具体原因可通过 `get_run()` 查看。`Succeeded` 只表示任务 owner 判定完成，运动证据仍由运动/导航状态判断。

## 任务运行订阅与定位状态

`tasks().watch_run(run_id)` 对应 `Tasks.Watch` 服务端流。Gateway 以目标 50Hz（20ms）读取并发送当前 AgentPlanner run 快照，状态不变也会发送；重复快照不会生成虚假的源采样时间。状态为 `done`、`failed`、`cancelled`，或 run 已不再保留时结束。丢弃订阅结束观察，取消任务使用 `cancel_run()`。普通查询超时与 metadata 仍按共享配置处理，长期订阅沿用 metadata 并由调用方管理持续时间。

```rust,no_run
use doso_sdk::{DosoClient, TaskRunStatus};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    let mut runs = robot.tasks().watch_run("run-id-from-receipt").await?;
    while let Some(run) = runs.message().await? {
        if let Some(state) = run.state()? {
            println!("{:?} {}", TaskRunStatus::try_from(state.status), state.result);
        } else {
            println!("{:?}", run.details);
        }
    }
    let localization = robot.localization().state().await?;
    println!("{:?}", localization);
    Ok(())
}
```

`Run::state()` 复用单次查询的解析规则，映射 `running/done/failed/cancelled`，保留失败原因与摘要。进度、阶段和采样时间仍取决于真实 owner 来源。运行历史由 owner 的保留规则决定，当前接口读取当前 run。

`localization().state()` 对应 `Localization.State`，将现有定位状态映射为 `LocalizationState`，包含 `quality`、`source_state`、`map_name`、原因及可缺省 `ready/pose_fresh`。未知状态保留源字符串并映射 `Unknown`。源未提供采样时间时 `time` 保持缺失，原 `status()` 继续提供完整诊断快照。

## 站起与坐下

`motion().stand_up(request_id)` 与 `sit_down(request_id)` 对应 `Motion.StandUp` 和 `Motion.SitDown`，请求复用公共 `Command`，响应复用 `CommandReceipt`。机器人端经过既有命令 journal 与 motion guard，再调用 `NavCommands.stand/sit`，通过 nav DDS、driver 到 Brainstem。

```rust,no_run
use doso_sdk::{new_request_id, DosoClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    let request_id = new_request_id()?;
    let receipt = robot.motion().stand_up(Some(&request_id)).await?;
    println!("accepted={:?} reason={}", receipt.accepted, receipt.reason);
    println!("{:?}", robot.motion().body_state().await?);
    Ok(())
}
```

坐下使用 `robot.motion().sit_down(Some(&request_id)).await?`，每次新操作使用独立请求 ID。姿态请求超时采用 `max(Config.timeout, 15秒)`，为已有停止屏障与 driver ACK 留出等待时间。回执描述请求接纳，身体状态查询与 `watch_body_state()` 提供后续源状态。ACK 未知时保留请求身份与未知提交结果。SDK、Gateway 和原生姿态链已通过本地构建与合同验证，设备动作待现场验收。

原生 ACK 超时、已请求取消、底层动作 ACK 未知及姿态发布失败，统一返回 `UNAVAILABLE` 和 `posture_ack_unknown`，错误保留未知提交事实。调用方应保留原请求 ID 核查源状态。

当前底层在没有 walking lease 的 Grounded/Transitioning 状态下，stop 可能返回 `lease_missing`。调用方应检查停止或连接清理回执，并结合身体状态核查实际结果。姿态接纳 ACK 与姿态到位分别判断，物理停稳和实际动作需设备验收。

导航任务 `navigation().watch(task_id)` 同样采用目标 50Hz 快照，终态结束；`wait()` 继续消费同一条流。服务端用绝对节拍扣除处理耗时，严重滞后时跳过错过的节拍。实际频率取决于运行负载与网络。

## 受控遥操作

`motion().teleop()` 一次握手建立双向流，首次有效速度请求经过既有 native 仲裁。持续控制使用 `publish_velocity()`：提交调用者当下的新输入，不等待逐条 ACK。后台独立接收窗口和回执，`latest_reply()` 返回最近一条回执；慢调用者可能跳过中间回执。

机器人每 50ms 签发一个有效期固定为 350ms 的输入窗口。同一窗口可携带多条递增序号的新输入，每个样本在调用时绑定窗口，排队后不会重新标记时间。旧窗口过期后不能因后续通信恢复有效。SDK 发送队列只有一个待发送位置，满时返回 `RESOURCE_EXHAUSTED`，不会把未入队的速度报告为成功；机器人入口与 native 发布器合并积压速度，保留最新值。

以下函数每 20ms 从调用者读取当前操作者输入；读取函数返回 `None` 或释放 deadman 时关闭会话。SDK 不会在后台自动重发上一条速度。

```rust,no_run
use doso_sdk::{DosoClient, TeleopVelocity};
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
        let request_id = session.publish_velocity(velocity, None)?;
        if let Some(reply) = session.latest_reply()? {
            println!("sent={request_id}, ack={} accepted={}", reply.request_id, reply.accepted);
        }
    }
    let reply = session.close(None).await?;
    if !reply.accepted {
        return Err(format!("release unconfirmed: {}", reply.reason).into());
    }
    Ok(())
}
```

`publish_velocity()` 返回本地入队的请求 ID，机器人接纳仍由回执的 `accepted/ingress_accepted` 表达。需要等待某一条速度回执时保留 `send_velocity(...).await`，它直接发送速度并等待应答，不再进行额外输入握手；持续控制应使用不等待 ACK 的发布方法。线速度单位为 m/s，机体 Z 轴角速度单位为 rad/s。

`hold()` 和 `close()` 等待真实回执，`final_output_confirmed` 表达 native 最终输出屏障。停止、deadman 释放或过期处理会废弃旧窗口，后续新输入使用回执或推送中的新窗口。机器人端连续 3 秒无客户端输入时结束连接，服务端的窗口推送不延长这个期限。取消等待或丢弃会话会关闭流，调用者需要确认释放时应显式 `close()` 并检查回执。

SDK 和 Gateway 需要一起更新连续输入协议：请求/回执使用递增 `sequence`，未支持该合同的旧 Gateway 会在开流阶段被拒绝。本地频率与延迟注入结果见[推进记录](grpc-v1-progress.md)，尚未代表真实机器人、无线网络或硬实时保证。

## 速度与里程计反馈

`motion().velocity()` 和 `odometry()` 读取现有SLAM估计反馈，`watch_odometry()` 按10Hz（每100ms）推送里程计快照，`watch_velocity()` 订阅目标最高100Hz的速度缓存快照。`source="localization"` 标明Gateway定位里程计输入，real原生路径由SLAM估计提供。生产者须明确标记六轴反馈可用，默认构造的Twist保持未知。速度字段含三轴线速度m/s和三轴角速度rad/s，表达在 `frame_id` 或 `child_frame` 中。位置、四元数表达子坐标系在 `parent_frame` 中的位姿，当前native SLAM常用 `map → body`。

```rust,no_run
use doso_sdk::DosoClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    println!("{:?}", robot.motion().velocity().await?);
    println!("{:?}", robot.motion().odometry().await?);
    let mut samples = robot.motion().watch_odometry().await?;
    while let Some(sample) = samples.message().await? {
        println!("source={} parent={} child={} position={:?} linear={:?} angular={:?} age_ms={:?}",
            sample.source, sample.parent_frame, sample.child_frame,
            sample.position_m, sample.linear_velocity_m_s, sample.angular_velocity_rad_s,
            sample.received_age_ms);
    }
    Ok(())
}
```

需要运行中的SLAM里程计输入。没有数据、字段不完整、定位已失效或接收缓存过期时返回 `UNAVAILABLE`，订阅在源失效时结束。`received_age_ms` 衡量Gateway接收后的年龄，`fresh`沿用现有定位诊断和2秒缓存年龄阈值。快照可重复，发布频率与SLAM估计更新率分别观察。当前real默认状态导出为10Hz，适配器最短50ms轮询，因此源新样本默认约10Hz。里程计按已确定的10Hz合同交付，速度反馈的新样本更新率也受同一源约束。

当前缓存的源 `ts` 没有明确时钟域，`SampleTime` 保持缺失。SDK保留完整四元数与三轴反馈，Brainstem本体尚未提供完整机身线速度和局部里程计。SLAM估计反馈与控制指令分别由其对应接口提供。

可运行只读例子 `cargo run --example read_motion -- http://<robot>:5052`，使用 `DOSO_API_KEY` 传递连接凭据。

## 运动状态订阅

`motion().watch_state()` 建立 gRPC 服务端流，持续读取与 `motion().status()` 相同来源的导航与控制状态快照。SDK 只发送一次订阅请求。机器人端目标最高推送频率为 100 Hz；这是快照读取频率，不是传感器采样频率或硬件事件通知。机器人端必须部署包含此调整的服务；旧版行为可能不同或返回 `UNIMPLEMENTED`。流沿用连接 metadata，订阅持续时间由调用方管理，普通请求的默认 10 秒超时不会截断该流。

`motion().watch_state_at(frequency_hz)` 可选择整数 1–100 Hz 的目标最高更新频率，例如 `watch_state_at(100)`。超出范围由机器人端返回 `INVALID_ARGUMENT`；旧版本可能只支持 1–4 Hz 或返回 `UNIMPLEMENTED`。机器人端将快照生成和发送耗时计入间隔；处理、网络和数据源更新频率可能降低实际频率。此设置只影响状态快照的读取和推送，不改变 IMU、关节等传感器的采样频率。快照中的 `ts` 是状态评估时间，不是传感器采样时间。

```rust,no_run
use doso_sdk::DosoClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    let mut states = robot.motion().watch_state_at(100).await?;
    while let Some(state) = states.message().await? {
        println!("{:?}", state.fields);
    }
    Ok(())
}
```

每次 `message()` 交付下一份快照，流内错误以 `tonic::Status` 返回。丢弃 `states` 会结束订阅。运动停止仍通过显式停止操作控制。该快照沿用已有状态字段，IMU、关节源事件通过观测领域订阅；订阅名额由这些流共同使用。

## 身体状态

`motion().body_state()` 单次读取 Brainstem `GetCmsState`，`motion().watch_body_state()` 订阅 `ListenCmsState`，先接收当前身体状态，随后接收源状态事件。`BodyState` 包含 `kind`、`transition` 和 `gesture_name`。`BodyKind` 定义 Zero、Grounded、Standing、Walking、Transitioning，`BodyTransition` 定义 None、StandUp、SitDown、Gesture。生成消息中的枚举字段保留原始 `i32` 值，调用方可使用 `BodyKind::try_from(state.kind)` 识别已知值，源端新增枚举值会原样传递。

```rust,no_run
use doso_sdk::{BodyKind, DosoClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    let state = robot.motion().body_state().await?;
    println!("{:?}", BodyKind::try_from(state.kind));
    let mut states = robot.motion().watch_body_state().await?;
    while let Some(state) = states.message().await? {
        println!("{state:?}");
    }
    Ok(())
}
```

身体状态描述底层有限状态机。导航任务和动作请求的完成结果仍通过各自回执和任务查询取得。身体状态事件没有固定推送频率，也没有源采样时间；丢弃流会结束本地订阅。单次查询与订阅建立错误通过 SDK `Error` 返回，流内错误通过 `tonic::Status` 返回。旧 Gateway 缺少对应 RPC 时返回 `UNIMPLEMENTED`。

`motion().control_status()` 只读获取 Brainstem 的身体状态、电机输出门、关键故障、源端 owner、lease 状态和计数。`BodyControlOwner` 数值与源枚举对应，未知值原样保留；GRPC owner 可能是机器人驱动，不能据此判断 SDK 调用方获得了控制权。该方法不申请或续约租约。

只读示例：`cargo run --example watch_body -- http://机器人地址:5052`。`motion().stop()` 请求普通停止；`emergency_stop()` 锁定急停；`clear_emergency_stop()` 请求复位急停，不恢复运动。回执不表示机器人已经物理停稳，设备必须部署包含对应 RPC 的服务。

## 地图位姿与导航路线流

`localization().watch_pose()` 持续读取地图坐标系的位姿缓存；`navigation().route()` 和 `watch_path()` 返回地图坐标系的 XYZ 路线，旧 `path()` 保留。流以 100 Hz 为最高目标，接收频率和源更新频率是不同的；本机夹具未达到 100 Hz，机器狗端负载与速率尚未验证。旧兼容路径可能在写入缓存时将缺失高度填零，不能把每个 z 都视为实测高度。

`PoseSample.received_age_ms` 是机器人服务收到位姿后的年龄，不是硬件采样年龄；未知时为 `None`。`fresh` 复用既有定位诊断与接收年龄判定，`watch_pose()` 在已知过期或定位丢失时返回 `UNAVAILABLE`；单次 `pose()` 保留读取最后已知位姿的行为，调用方应检查 `fresh`。

`navigation().pause(task_id, reason, request_id)` 与 `resume(...)` 控制指定任务，恢复仍通过现有运动检查。请求 ID 必须区别于任务 ID；回执接纳后，通过 `task()` 或 `watch()` 确认实际状态。复位急停不是恢复导航任务。

只读示例：`cargo run --example watch_navigation -- http://机器人地址:5052`。旧机器人服务缺少新增 RPC 时返回 `UNIMPLEMENTED`。

## IMU 与关节订阅

`observations().watch_imu()` 和 `watch_joints()` 建立 gRPC 服务端流，接收机器人 Brainstem 的源事件。请求沿用连接 metadata，流的持续时间由调用方管理。每次 `message()` 返回一条 `ImuSample` 或 `JointSample`，丢弃流即可结束订阅。

仓库自带只读示例，同时接收两条流，各打印最多 100 条源事件。将地址换成已部署新 Gateway 的机器人地址：

```text
cargo run -p doso-sdk --example read_observations -- http://机器人地址:5052
```

```rust,no_run
use doso_sdk::DosoClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    let observations = robot.observations();
    let (mut imu, mut joints) = tokio::try_join!(observations.watch_imu(), observations.watch_joints())?;

    // 同时消费两条流，完成有限次读取后结束订阅。
    let read_imu = async {
        for _ in 0..100 {
            let Some(sample) = imu.message().await? else { break };
            println!("IMU: {sample:?}");
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    };
    let read_joints = async {
        for _ in 0..100 {
            let Some(sample) = joints.message().await? else { break };
            for joint in sample.joints {
                println!("joint {} position {} rad", joint.id, joint.position_rad);
            }
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    };
    tokio::try_join!(read_imu, read_joints)?;
    Ok(())
}
```

IMU 消息提供角速度 `angular_velocity_rad_s`、世界到机身方向的四元数 `orientation_world_to_body` 和线加速度 `linear_acceleration_m_s2`。这些 Protobuf 消息字段在 Rust 中为 `Option`；线加速度缺失时保持 `None`，源数据缺少角速度或姿态时 Gateway 返回错误。关节消息包含源端提供的关节 ID、位置 rad、速度 rad/s、力矩 N·m 和原始状态码。

每份 `JointSample` 对应一个源事件，可以包含单个关节，也可以包含上游完整帧中的全部关节。调用方按 `id` 识别关节，ID 的实际对应关系由机器人型号决定。消息保留源事件边界，不能把单关节事件当作同步采集的全身快照。

`source_elapsed` 是可缺省的 Brainstem 服务相对时长，记录源事件映射时间。它只适用于同一服务生命周期内的时间比较。IMU 和关节流按源事件推送，当前接口没有选频或重采样参数。实际接收频率受源数据、服务处理与网络影响，100 Hz 需要在对应实机链路上测量确认。机器人端需要支持这两个 RPC 的 Gateway 并接通 Brainstem 观测源，旧版 Gateway 返回 `UNIMPLEMENTED`。

建立订阅的错误以 SDK `Error` 返回，流内读取错误以 `tonic::Status` 返回，调用方可以直接保留源端的失败原因。

## 电机诊断查询

`observations().motors()` 单次查询 Brainstem 的电机状态，返回 `MotorStates.motors`。每项包含 `id`、`online`、原始 `status_code`、`temperature_c`（°C）和原始故障码列表 `errors`。`online` 表示源服务观察到该电机近一秒有通信。源服务缺少反馈报告时会返回温度和状态码 0、空故障列表，离线电机同样返回这些默认值。`online` 本身没有证明反馈报告存在，调用方不能仅凭默认温度或空故障列表判断电机健康。当前协议没有反馈报告有效位或采样时间。

`observations().motor_voltages()` 单次调用 Brainstem 独立的 `GetVoltage`，返回 `MotorVoltagesResponse.volts`，单位 V，数组按关节索引排列。当前 Thunder 为 16 项，索引 0–11 依次为 FR、FL、RR、RL 的 hip/thigh/calf，12–15 为对应 foot。源服务向电机发起只读总线电压查询，默认等待 500 毫秒，未响应的电机返回 0。响应没有逐轴有效位，SDK 原样保留该 0 值。该查询适合按需读取，调用频率会影响 CAN 总线查询流量。

电机状态接口中的固定电压占位没有投影到 `MotorState`，电压通过上述独立查询取得。两个方法沿用普通请求超时与 metadata，上游失败按 SDK `Error` 返回。电池电量仍缺少生产数据源。

## 导航与结果

导航客户端的目标、任务、暂停/恢复/取消与路线接口已有实现，并已接到 LingTu 导航后端的本地 gRPC 调用链。真实机器狗的正式 `5052` 服务及导航到点闭环仍待部署验收。

可运行示例 [`navigate`](../examples/navigate.rs) 提交一次地图坐标目标并等待结果：

```text
cargo run --example navigate -- --help
cargo run --example navigate -- http://<robot>:5052 3 2 0 1.57 120
```

第二条命令会实际发送导航目标，需运行中的导航服务、可用地图与定位、控制权限和现场安全条件。参数为 `endpoint x y z yaw [timeout_seconds]`，位置单位 m、偏航角单位 rad；等待默认120秒，正数检查在提交前完成。认证使用 `DOSO_API_KEY`，私有 CA 使用 `DOSO_TLS_CA_FILE`。

示例打印请求和任务身份，拒绝、未知提交、失败或超时以非零退出码结束；不自动重试或取消。`--help` 不连接设备。完整方法与失败处理见[导航 API](api/navigation.md)。

以下示例会提交运动目标；应在已确认的地图、定位和操作环境中运行。

```rust,no_run
use doso_sdk::{new_request_id, DosoClient, Pose};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    let request_id = new_request_id()?;
    let receipt = robot.navigation().go_to(
        Pose { x: 3.0, y: 2.0, z: 0.0, yaw: 1.57 }, Some(&request_id),
    ).await?;
    if receipt.accepted == Some(true) {
        if let Some(task_id) = receipt.task_id {
            let result = robot.navigation().wait(&task_id, Duration::from_secs(120)).await?;
            println!("{result:?}");
        }
    } else {
        eprintln!("{}", receipt.reason);
    }
    Ok(())
}
```

`navigation().watch(task_id)` 返回服务端任务状态流；`wait(task_id, timeout)` 消费这条流，按同一任务的终态返回结果。它结束或超时只取消本地等待，不取消远端导航任务。取消任务使用 `navigation().cancel(...)`；软件急停使用 `motion().emergency_stop(...)`。

当前每个 Gateway 的遥操作、运动、身体、导航、任务、位姿、路线、IMU、关节、相机、实时点云订阅和地图下载共享最多 8 个流名额，超额返回 `RESOURCE_EXHAUSTED`。流结束后释放名额，为同步服务中的普通请求保留执行线程。

`CommandReceipt` 是协议中 `Receipt` 的公共别名，包含接纳、请求和任务身份、`reason_code`、说明及阶段。`accepted` 明确给出时以它判断接纳；旧 `ok` 保留为兼容投影。接纳与任务完成分别表达。

`Error::code()` 返回 SDK 导出的标准 `Code`；`reason_code()` 读取执行方给出的稳定原因，`details()` 保留原始错误响应。错误详情通过 `doso-error-bin` 元数据传递，其消息由 `proto/messages/error.proto` 定义；没有该元数据的服务仍可按标准 gRPC 码处理。元数据损坏会在错误说明中明确报告，并保留原 RPC 码。

`navigation().task()` 成功读取到失败或取消任务时返回该 `Task`。`wait()` 对非成功终态返回错误，完整结果保留在 `error.terminal`：`Terminal::Navigation`、`Terminal::Map` 或 `Terminal::Product`，可直接读取原状态和身份。`reason_code()`、`details()` 从此结果派生，不额外维护一份终态。合法 `UNKNOWN` 表示尚无确定结果，等待继续受总超时约束。

`Error` 保留 `request_id` 和 `submission_unknown`；客户端不会自动重发写命令。各返回码与状态语义见 [返回码与状态](results.md)。

地图保存使用 operation_id 查询和等待，保存接纳与持久化成功分别表达。地图加载和新建图通过 ProductControl，要求显式传入预期 product_session_id。加载地图定位不表示支持恢复旧优化图跨会话续建。定位重定位只操作当前活动地图；它不是地图切换。地图位姿来自 SLAM/定位服务，Brainstem 身体状态不作为地图位姿。

## 地图点云与相机图像

`maps().grid(map_name)` 读取 mapd 保存的二维占据栅格，返回地图 ID、内容代次、地图坐标系、分辨率、原点、尺寸和行优先 cells。0–100 表示占据值，255 表示未知；读取不激活地图。它是点云投影的二维数据，不能把未知区域当作自由空间。源只有二维原点，z 不代表三维高度；采样时间未知。

只读示例：`cargo run --example read_grid -- http://机器人地址:5052 地图名`。

`observations().camera_sample(token)` 和 `watch_camera_samples(token)` 提供 JPEG 数据及已有元信息。流中 `sequence` 是机器人服务发布序号，重启后重置，并非硬件帧号；单帧查询无序号，采样时间、相机 ID 和尺寸没有来源时保持未知。旧 `camera_frame()`、`watch_camera()` 继续可用。

相机流的首帧必须在订阅后发布；单次快照仍允许读取缓存。新发布可能来自编码器重新编码已有原始图像，因此不保证硬件新采样。`CameraInfo` 的 ID、宽高和坐标系缺失时为 `None`，不以 0 表示未知尺寸。

`observations().watch_point_cloud(CloudKind::Map)` 订阅实时地图显示帧，`CloudKind::Scan` 订阅当前扫描显示帧。两者来自现有 CloudViewerService，使用 PCLD v2 二进制编码。调用 `CloudFrame::decode()` 可取得米制 XYZ、可选 RGB、`frame_id`、`epoch`、`sequence`、`stamp_s` 和 `stream_kind`。

```rust,no_run
use doso_sdk::{CloudKind, CloudStreamKind, DosoClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    let mut frames = robot.observations().watch_point_cloud(CloudKind::Scan).await?;
    while let Some(frame) = frames.message().await? {
        let cloud = frame.decode()?;
        if cloud.stream_kind == CloudStreamKind::Reset {
            println!("clear scene for epoch {}", cloud.epoch);
        } else {
            println!("{} points in {}", cloud.points.len(), cloud.frame_id);
        }
    }
    Ok(())
}
```

默认地图显示通道最高约 4 Hz、每帧最多 60,000 点，扫描通道最高约 10 Hz、默认最多 15,000 点；实际更新由源数据和发布规则决定。源端进行量化、体素处理或采样，`scale_m` 表示该帧量化步长。首帧可能来自缓存，慢消费者可能跳过旧帧。时间沿用现有显示源的 `stamp_s`，源时间缺失时可能回退 Gateway 时间。叠加点云应匹配 `frame_id` 和 `epoch`，reset 事件应清空对应显示内容。

此 RPC 单帧解码上限为 10 MiB，可覆盖当前支持的一百万点扫描。`decode()` 接受 PCLD v2，其他版本返回协议错误。丢弃订阅会结束流，旧 Gateway 返回 `UNIMPLEMENTED`。

订阅建立不代表源已有新数据；没有帧时 `message()` 会等待，调用方可自行设置等待超时。仓库中的只读示例持续打印帧信息，使用 Ctrl+C 结束：

```text
cargo run -p doso-sdk --example watch_cloud -- http://机器人地址:5052 scan
cargo run -p doso-sdk --example watch_cloud -- http://机器人地址:5052 map
```

`maps().point_cloud(name)` 从 mapd 读取已保存地图的原始 PCD，返回服务端分块流。首条消息是地图 ID、可选内容版本、可选坐标系和字节数，后续消息是 PCD 字节块；应读取到正常结束并检查字节总数。它不切换产品或激活地图，也不解析或渲染点云。可用只读示例保存到新文件，再用支持 PCD 的工具查看：

```text
cargo run -p doso-sdk --example download_map -- http://机器人地址:5052 地图名 map.pcd
```

示例不会覆盖已有文件，读取失败会尝试删除本次创建的不完整文件。相机 JPEG 快照、持续帧及媒体权限用法见[相机图像和远程观看](camera.md)。

## 机器人端与协议生成

LingTu 的 `GatewayModule` 生命周期拥有 gRPC 服务，默认端口 **5052**；Web 继续使用现有 HTTP **5050**。gRPC handler 与 HTTP 路由调用同一份领域操作，保留现有任务 journal、控制权和执行 owner。

在 LingTu 运行环境安装 Gateway 依赖：

```text
python -m pip install -e ".[gateway]"
```

随后沿用 LingTu 的 ProductControl 启动流程；包含 Gateway 的 Host 会同时启动 gRPC listener，无需另起 SDK 服务进程。Python 生成绑定已随代码提供，只有修改协议时才需要重新生成。

ProductControl 是独立生命周期服务，其现有本机 **5051** 传输继续沿用。因此 SDK 到 Gateway 已使用 gRPC，并不表示机器人内部每一条链路均已改成 gRPC。

协议统一维护于 `proto/`。协议变更后，从 DOSO-SDK 根目录生成 LingTu 的 Python 绑定：

```text
python scripts/generate_grpc.py --lingtu ../lingtu
python scripts/test_generate_grpc.py
```

生成工具需要 `grpcio-tools`；SDK 协议输出到 LingTu 的 `src/gateway/rpc/generated/`，Brainstem 协议输出到 `src/runtime/adapters/brainstem/generated/`。不要分别维护第二套协议或手改生成绑定。

## 当前边界与验证

站起/坐下请求、受控遥操作、任务状态快照流与类型化定位状态已接 SDK 和 Gateway，原生姿态链构建与本地合同测试通过，具体验证范围见推进记录。实机动作、电池电量/充电状态、光照、独立于SLAM的机身速度/局部里程计等来源、综合健康与统一故障、单独控制权管理、断线恢复及统一巡检/跟随继续推进。地图内容接口读取已保存的 PCD，实时点云读取显示帧，相机提供 JPEG 流。现有 Brainstem C++ 客户端继续服务底层运动链路。

```text
cargo run -p doso-sdk --example read_status -- http://127.0.0.1:5052
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo package -p doso-sdk --allow-dirty --locked
```

只读示例可从 `DOSO_API_KEY` 读取 metadata；`--help` 不连接服务。SDK、真实 Gateway 接线、仿真和实机证据分别记录，具体结果见仓库 [gRPC 推进记录](grpc-v1-progress.md)。
