# localization 定位与地图位姿

通过 `robot.localization()` 取得 `Localization<'_>`。`Result<T>` 为 `doso_sdk::Result<T>`，消息位于 `doso_sdk::rpc`。本领域读取 LingTu 定位结果，并提供当前活动地图的重定位请求。

源码与消息见 [Localization](../../src/localization/mod.rs)、[类型化状态](../../src/localization/state.rs)和[定位消息](../../proto/messages/localization.proto)。

## 方法索引

| 方法 | 用途 | 调用方式 |
| --- | --- | --- |
| [status](#status) | 完整定位诊断 | 单次查询 |
| [state](#state) | 类型化定位状态 | 单次查询 |
| [pose](#pose) | 最后已知 map 位姿 | 单次查询 |
| [watch_pose](#watch_pose) | map 位姿缓存快照 | 服务端流 |
| [location](#location) | WGS84 地理位置与 GNSS 可用状态 | 单次查询 |
| [watch_location](#watch_location) | 地理位置与无定位、过期、恢复状态 | 1Hz 服务端流 |
| [relocalize](#relocalize) | 在当前活动地图重定位 | 写请求 |

## 公共调用约定

前置条件为已连接配套 Gateway。普通查询采用 `Config.timeout`，默认 10 秒。建立订阅使用 SDK `Error`，流内 `message().await` 使用 `tonic::Status` 返回错误。凭据、超时、服务兼容与依赖不可用遵循[返回码与状态](../results.md)。

## 定位状态

### status

```text
pub async fn status(&self) -> Result<rpc::Snapshot>
```

无额外参数。调用 `Localization.Status`，返回现有 SLAM/定位服务的完整诊断投影。`Snapshot.fields` 为可扩展 `Struct`，具体字段与来源由机器人端维护。读取失败、连接失败或超时返回 SDK 错误。

### state

```text
pub async fn state(&self) -> Result<rpc::LocalizationState>
```

无额外参数。调用 `Localization.State`，返回定位 owner 的类型化状态。

| 字段 | 语义 |
| --- | --- |
| `quality` | 定位质量枚举的原始 `i32`，可用 `LocalizationQuality::try_from` 识别 |
| `source_state` | 现有 owner 的原始状态字符串 |
| `map_name` | 已知活动地图名称 |
| `ready`、`pose_fresh` | 可缺省布尔值，未知时为 `None` |
| `reason` | 定位状态原因 |
| `time` | 当前没有明确源采样时间，保持缺失 |

未知源状态映射为 `Unknown` 并保留 `source_state`。成功读取到 `Lost` 表示查询取得了定位丢失这一事实，调用方继续检查返回数据。RPC 本身失败时返回 SDK 错误。

## 地图位姿

### pose

```text
pub async fn pose(&self) -> Result<rpc::PoseSample>
```

无额外参数。调用 `Localization.Pose`，读取 Gateway 保存的最后已知 map 位姿。

| 字段 | 单位与可选语义 |
| --- | --- |
| `pose` | `Option<Pose>`，其中 x/y/z 为米，yaw 为弧度 |
| `frame_id` | 当前有效返回为 `"map"` |
| `received_age_ms` | Gateway 收到该位姿后的年龄，单位毫秒，未知时 `None` |
| `fresh` | 已有定位诊断与接收年龄的综合结果，未知时 `None` |
| `details` | 原始位姿缓存补充信息 |

单次查询允许读取最后已知位姿，调用方应检查 `fresh`。机器人端没有可用 map 位姿时返回 `UNAVAILABLE`。SDK 对响应中的 map 坐标系、缺失 pose 和非有限坐标执行检查，异常响应返回协议错误。

### watch_pose

```text
pub async fn watch_pose(&self) -> Result<tonic::Streaming<rpc::PoseSample>>
```

无额外参数。调用 `Localization.WatchPose`，持续接收 map 位姿缓存快照，目标最高 100Hz。源位姿更新率、快照接收率和源采样率分别由对应环节决定，重复快照合法。

订阅沿用共享 metadata，持续时间由调用方管理。每次 `message().await` 读取下一条，`None` 表示流正常结束，丢弃流会释放订阅。已知过期或定位丢失时，机器人端以 `UNAVAILABLE` 结束流。订阅名额与其他 SDK 长流共用，超额为 `RESOURCE_EXHAUSTED`。`received_age_ms` 描述接收年龄，源采样时间仍需按来源解释。

```rust,no_run
use doso_sdk::{DosoClient, LocalizationQuality};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    let localization = robot.localization();
    println!("{:?}", localization.status().await?);
    let state = localization.state().await?;
    println!("{:?}", LocalizationQuality::try_from(state.quality));
    let sample = localization.pose().await?;
    println!("pose={:?} fresh={:?}", sample.pose, sample.fresh);
    let mut samples = localization.watch_pose().await?;
    for _ in 0..3 {
        let Some(sample) = samples.message().await? else { break };
        println!("{:?}", sample);
    }
    Ok(())
}
```

## 地理位置

### location

```text
pub async fn location(&self) -> Result<rpc::GeographicLocation>
```

调用 `Localization.Location`，读取已有 GNSS 进程的快照，不打开接收器，不启用云上传。地理位置采用 WGS84 经纬度，与 `pose()` 的 map 坐标分别表示。

`state` 为 `GeographicState`，取值包括 Unknown、Ready、Unavailable、NoFix、Stale 和 Invalid。`GeographicLocation`、`GeographicFix`、`GeographicState` 均由 `doso_sdk` 直接导出。

| 字段 | 类型与单位 | 含义 |
| --- | --- | --- |
| `state` | `GeographicState` | 定位可用性 |
| `fix` | 可选 `GeographicFix` | Ready 时的位置 |
| `age_s` | 可选浮点数，秒 | 读取时的源时间年龄 |
| `fix.latitude`、`fix.longitude` | 浮点数，度 | WGS84 纬度和经度 |
| `fix.fix_type` | 无符号整数 | 原始 NMEA fix 代码 1–8 |
| `fix.observed_at` | UTC `Timestamp` | 当前驱动接收该 fix 时记录的源时间 |
| `fix.accuracy_m` | 可选浮点数，米 | 实测水平精度，未知时缺失 |

只有 Ready 携带 `fix`。精度保持接收机实际提供的米数，未知精度保持缺失。`age_s` 在无法计算时缺失。

机器人端复用已有地理位置读取规则：默认读取 `/dev/shm/lingtu/gnss_status.json`，超过 3 秒为 Stale，未来超过 1 秒或无效文件为 Invalid，无有效定位为 NoFix。上述情况返回明确状态且不带旧坐标；文件不存在为 Unavailable。RPC 传输、鉴权或旧服务不支持的错误按原 gRPC 错误返回。

### watch_location

```text
pub async fn watch_location(&self) -> Result<tonic::Streaming<rpc::GeographicLocation>>
```

调用 `Localization.WatchLocation`，每秒读取一次已有 GNSS 快照，持续发送位置及失效、恢复状态；这是订阅快照频率，不提高接收器采样率。重复快照允许，`observed_at` 始终保留源时间。无定位不会结束流；取消、连接错误及共享订阅名额沿用公共规则。此流同样不启用任何云功能。

仓库中的 `watch_location` 示例连接后先单次读取，再订阅 1Hz 流，输出状态、经纬度、原始源时间、定位类型、可选精度和数据年龄。在 SDK 根目录执行以下命令，将地址换为机器狗运行 LingTu gRPC 的实际地址。配置了 API key 的服务从 `DOSO_API_KEY` 环境变量读取凭据。

```powershell
cargo run --example watch_location --locked -- http://192.168.1.100:5052
```

`Ready` 返回有效 `fix`。`NoFix`、`Stale`、`Unavailable` 或 `Invalid` 返回对应状态并省略坐标，流会继续等待恢复。按 Ctrl+C 结束示例。这里读取本地 SDK 服务，云端上报由机器狗上的 `doso-cloud-link` 根据已授权的状态许可执行。

```rust,no_run
use doso_sdk::DosoClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    println!("{:?}", robot.localization().location().await?);
    let mut locations = robot.localization().watch_location().await?;
    while let Some(location) = locations.message().await? {
        println!("state={} fix={:?} age_s={:?}", location.state, location.fix, location.age_s);
    }
    Ok(())
}
```

## 重定位

### relocalize

```text
pub async fn relocalize(
    &self,
    map_name: &str,
    initial_pose: Option<Pose>,
    request_id: Option<&str>,
) -> Result<CommandReceipt>
```

调用 `Localization.Relocalize`，请求在当前运行产品的活动地图上重新定位。

| 参数 | 含义与单位 |
| --- | --- |
| `map_name` | 目标活动地图，要求非空且符合设备端地图名称规则 |
| `initial_pose` | `Some(Pose)` 提供 map 坐标系初值，x/y/z 为米、yaw 为弧度。`None` 请求全局重定位 |
| `request_id` | 非空请求身份。`None` 由 SDK 生成，显式传入可用于核查同一次操作 |

前置条件为地图已由 ProductControl 激活，定位服务支持相应重定位方式。此方法固定使用 60 秒请求超时。返回 `CommandReceipt`，调用方读取接纳、原因与原始详情，并结合后续 `state()` 和 `pose()` 检查定位结果。地图切换接口见 [maps](maps.md)。

空名称、非有限初值或无效请求 ID 在本地返回输入错误。地图不匹配、模式或能力条件不满足、定位源不可用、重定位超时等失败保留机器人端错误。写入结果不明确时保留 `request_id` 与 `submission_unknown`，具体语义见[返回合同](../results.md)。

以下示例会请求重定位。地图名和初值应来自当前活动地图。

```rust,no_run
use doso_sdk::{new_request_id, DosoClient, Pose};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    let request_id = new_request_id()?;
    let receipt = robot.localization().relocalize(
        "lab",
        Some(Pose { x: 1.0, y: 2.0, z: 0.0, yaw: 0.0 }),
        Some(&request_id),
    ).await?;
    println!("accepted={:?} reason={}", receipt.accepted, receipt.reason);
    println!("{:?}", robot.localization().state().await?);
    Ok(())
}
```

[返回 API 索引](README.md)
