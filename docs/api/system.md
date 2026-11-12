# system 系统与就绪

通过 `robot.system()` 取得 `System<'_>`。本领域提供身份快照与系统查询，方法中的 `Result<T>` 为 `doso_sdk::Result<T>`，消息位于 `doso_sdk::rpc`。

源码与消息定义分别见 [System](../../src/system/mod.rs)、[类型化状态](../../src/system/status.rs)、[就绪转换](../../src/system/readiness.rs)和[系统消息](../../proto/messages/system.proto)。

## 方法索引

| 方法 | 用途 | 调用方式 |
| --- | --- | --- |
| [info](#info) | 最近成功握手的三项身份信息 | 同步本地借用 |
| [ping](#ping) | 检查服务响应 | 单次 RPC |
| [state](#state) | 系统运行摘要 | 单次 RPC |
| [robot_status](#robot_status) | 类型化模式、就绪和急停信息 | 单次 RPC |
| [health](#health) | 模块与传感器健康摘要 | 单次 RPC |
| [faults](#faults) | 已知活动故障及来源覆盖 | 单次 RPC |
| [watch_faults](#watch_faults) | 故障与覆盖状态快照 | 服务端流 |
| [readiness](#readiness) | 原始就绪诊断 | 单次 RPC |
| [readiness_state](#readiness_state) | 类型化就绪字段 | 单次 RPC 与本地转换 |
| [capabilities](#capabilities) | 当前运行配置的能力信息 | 单次 RPC |
| [session](#session) | 当前产品会话 | 单次 RPC |
| [product_status](#product_status) | ProductControl 状态 | 单次 RPC |

## 公共调用约定

下列查询均无额外入参，前置条件为已成功完成 `DosoClient` 连接与握手。普通请求使用 `Config.timeout`，默认 10 秒。查询失败以 SDK `Error` 返回，`error.code()` 保留标准 gRPC 码。凭据、连接、超时及服务兼容错误可分别表现为 `UNAUTHENTICATED`、`UNAVAILABLE`、`DEADLINE_EXCEEDED`、`UNIMPLEMENTED`。具体分类见[返回码与状态](../results.md)。

`Snapshot.fields` 为可缺省的 `google.protobuf.Struct`。2026-11-12 起配套服务按公开字段投影系统查询，不再原样转发内部端点清单、运行配置、进程证据、文件路径和完整操作记录。缺少的 optional 布尔值对应 Rust `None`，调用方应保留它与 `Some(false)` 的区别。

SDK 提供机器人本地能力，不持有云管理凭据。设备只按云端许可上报自身数据，不能修改云端归属、权限、规则和管理配置。云链只提供已定义的状态与媒体能力，禁止运动指令、透传任意 gRPC 方法或隧道转发；本地 `motion` 等接口继续由机端控制权与安全链处理。

## 身份与连通性

### info

```text
pub fn info(&self) -> &'a rpc::SystemInfo
```

返回与 `System<'a>` 所借用客户端同寿命的最近成功握手快照。调用只借用本地数据；`reconnect()` 成功后刷新，失败则保留上次成功快照，不能据此判断当前在线。重连规则见[连接 API](client.md#reconnect)。

| 字段 | Rust 类型 | 来源与含义 |
| --- | --- | --- |
| `firmware_version` | `Option<String>` | 当前运行发布包的 `metadata.json.version` |
| `robot_version` | `Option<String>` | 机器人型号，来自 RunPlan 或 `LINGTU_ROBOT` |
| `robot_id` | `Option<String>` | 设备配置 `DOSO_DEVICE_ID`，规范小写 UUID v4 |

缺少来源时字段为 `None`。重新连接可取得新握手快照。配置机器人 ID 无效会在连接握手阶段返回 `FAILED_PRECONDITION` 与 `invalid_robot_id`，本方法本身无错误返回。发布与身份规则见[机器人信息与固件发布](../release.md)。

### ping

```text
pub async fn ping(&self) -> Result<rpc::Ping>
```

调用 `System.Ping`，返回服务的 `status` 字符串，当前 Gateway 正常响应为 `"ok"`。该结果描述本次服务响应。失败沿用公共查询错误约定。

## 状态与健康

### state

```text
pub async fn state(&self) -> Result<rpc::Snapshot>
```

调用 `System.State`，返回已有时间、会话、遥操作活跃、安全、定位及导航的公开运行摘要。各摘要按业务字段投影，不包含完整租约、内部端点、运行配置和源证据。查询可能受到依赖服务不可用或普通请求超时影响。

### robot_status

```text
pub async fn robot_status(&self) -> Result<rpc::RobotStatus>
```

调用 `System.RobotStatus`，读取当前模式、就绪与原因。软件急停仅在 native 状态新鲜且字段明确时提供。

| 字段 | 可用性与解释 |
| --- | --- |
| `mode` | 当前 Gateway 模式字符串 |
| `ready` | 可缺省的整体就绪值 |
| `software_emergency_stop` | 新鲜 native 状态中的软件急停锁存值，未知时 `None` |
| `health` | 当前整机状态投影保持 `HealthStatus::Unknown`，已知故障与覆盖通过 `faults()` 查询 |
| `hardware_emergency_stop` | 当前来源未知，保留 `None` |
| `time` | 当前缺少源采样时间，保持缺失 |
| `reason` | 现有就绪原因 |

健康和硬件急停的未知值是返回数据的一部分。查询仍可正常成功，RPC 失败按公共约定返回。

### health

```text
pub async fn health(&self) -> Result<rpc::Snapshot>
```

调用 `System.Health`，返回整体状态、模块通过/失败计数、里程计可用性、SLAM频率、地图点数、模块状态和传感器可用性/频率摘要。不返回 Gateway 运行统计、设备IP、状态文件路径或完整底层驱动记录。类型化整机健康的可用性由 `robot_status()` 单独表达。来源读取失败、超时等情况沿用公共查询约定返回。

### faults

```text
pub async fn faults(&self) -> Result<rpc::FaultReport>
```

读取当前已知活动故障和明确的来源覆盖情况。当前 Gateway 投影 `driver.control`、`driver.health`、`navigation.control_loop` 三类来源，分别使用新鲜的 driver 控制状态、driver 健康与 native 控制循环状态。来源缺失、过期或缺少必要字段时列入 `unavailable_sources`。

| `FaultReport` 字段 | 含义 |
| --- | --- |
| `faults` | 当前已知活动故障列表，每项含 source、code、severity、message、active 与可选 time |
| `observed_sources` | 本次读取中可用且满足该来源读取条件的来源 |
| `unavailable_sources` | 本次读取中不可用的来源 |
| `health` | 已知故障中有 Error 时为 Error，仅有 Warning 时为 Warning，没有已知故障时为 Unknown |

`driver.control` 上报 `critical_motor_fault`，`driver.health` 保留实际异常原因，`navigation.control_loop` 异常作为 Warning。重复的关键电机故障只保留一项。电机关闭属于正常操作状态，`motors_disabled` 本身不加入故障列表。空列表仍需结合覆盖字段判断，整机全部组件的健康覆盖尚待接入。

### watch_faults

```text
pub async fn watch_faults(&self) -> Result<tonic::Streaming<rpc::FaultReport>>
```

按目标最高 50 Hz 持续读取上述快照，各来源实际更新可能更慢。重复快照表达当前已知故障与覆盖，流中断通过 `tonic::Status` 返回。该流只保留当前投影，历史故障记录由应用按需维护。订阅沿用 API key，调用者通过丢弃流释放订阅。

源码见[故障查询与订阅](../../src/system/faults.rs)。

## 就绪与能力

### readiness

```text
pub async fn readiness(&self) -> Result<rpc::Snapshot>
```

调用 `System.Readiness`，只返回 status、ready、data_ready、motion_ready、non_motion_safe、reasons、advisories。它与 `readiness_state()` 使用同一 RPC，后者在客户端转换为类型化消息，不重复查询来源。查询成功后仍需读取内容判断是否就绪，失败按公共约定返回。

### readiness_state

```text
pub async fn readiness_state(&self) -> Result<rpc::ReadinessState>
```

调用 `readiness()` 并在 SDK 内转换为明确字段，无额外 RPC。

| 字段 | 含义 |
| --- | --- |
| `status` | 原始就绪状态字符串 |
| `ready`、`data_ready`、`motion_ready`、`non_motion_safe` | 各自的 `Option<bool>`，缺失保持 `None` |
| `reasons` | 就绪条件未满足的原因列表 |
| `advisories` | 提示信息列表 |

原始 `fields` 缺失、布尔字段类型错误、原因列表含非字符串等情况返回协议错误。远端读取失败保持原错误。

### capabilities

```text
pub async fn capabilities(&self) -> Result<rpc::Snapshot>
```

调用 `System.Capabilities`，只返回公开功能布尔值 `features` 与 `products` 中各运行模式的 `available`。不返回 HTTP/WebSocket 端点清单、验证探针、后端选择、RunPlan 配置或内部流量策略。功能声明不等于已取得控制权或硬件就绪，使用时仍需满足所属领域条件。失败按公共查询约定返回。

## 产品会话

### session

```text
pub async fn session(&self) -> Result<rpc::Snapshot>
```

调用 `System.Session`，只返回已有 `mode`、`active_map`、`product` 和必要的 `product_session_id`，不转发内部定位/地图装配诊断。地图切换的会话校验使用 `product_status()` 的类型化结果。失败按公共查询约定返回。

### product_status

```text
pub async fn product_status(&self) -> Result<rpc::ProductStatus>
```

调用 `System.ProductStatus`，由机器人端读取 ProductControl 状态，并在 SDK 返回 `available`、`status`、`product`、`product_session_id`。不返回进程证据、内部配置和最近其他操作的完整结果。`product_session_id` 是 SDK 提交地图/模式切换时必须原样携带的并发校验值，客户界面无需显示。未运行产品时 `product` 和会话值缺失。ProductControl 不可达保持 `UNAVAILABLE`，格式错误为 SDK 协议错误。

开发预览调用方从原 `Snapshot.fields.current` 取值的代码需改为直接读取上述字段；gRPC 线协议方法与请求不变。该方法不执行切换，也不改变云端配置。

## 完整查询示例

下例只读取系统信息。代码可编译，运行时地址需替换为已部署配套服务的机器人端点。

```rust,no_run
use doso_sdk::{DosoClient, HealthStatus};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    let system = robot.system();
    println!("{:?}", system.info());
    println!("{:?}", system.ping().await?);
    println!("{:?}", system.state().await?);
    let status = system.robot_status().await?;
    println!("health={:?} ready={:?}", HealthStatus::try_from(status.health), status.ready);
    println!("{:?}", system.health().await?);
    let faults = system.faults().await?;
    println!("faults={:?} unavailable={:?}", faults.faults, faults.unavailable_sources);
    println!("{:?}", system.readiness().await?);
    println!("{:?}", system.readiness_state().await?);
    println!("{:?}", system.capabilities().await?);
    println!("{:?}", system.session().await?);
    println!("{:?}", system.product_status().await?);
    Ok(())
}
```

[返回 API 索引](README.md)
