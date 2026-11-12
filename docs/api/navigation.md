# navigation 导航

通过 `client.navigation()` 访问导航领域。机器人侧导航 owner 负责目标、规划、任务生命周期和最终运动仲裁，SDK 负责提交请求、接收状态和关联任务身份。目标位姿使用 `map` 坐标系，位置单位为 m，`yaw` 单位为 rad。

本页签名中的 `Result<T>` 为 `doso_sdk::Result<T>`，`rpc` 为 `doso_sdk::rpc`。`CommandReceipt` 是协议 `Receipt` 的公共别名。参数、消息和错误详情分别由源码、协议及公共合同维护。[Rust 实现](../../src/navigation/mod.rs) · [任务控制实现](../../src/navigation/control.rs) · [协议](../../proto/navigation.proto) · [消息](../../proto/messages/navigation.proto) · [返回码与状态](../results.md)

## 方法目录

| 分组 | 方法 | 用途 |
| --- | --- | --- |
| 目标 | [`go_to`](#go_to) | 提交地图坐标目标，取得导航任务身份 |
| 状态 | [`status`](#status)、[`task`](#task) | 读取综合诊断或指定任务 |
| 订阅与等待 | [`watch`](#watch)、[`wait`](#wait) | 连续观察同一任务，或等待其终态 |
| 任务控制 | [`pause`](#pause)、[`resume`](#resume)、[`cancel`](#cancel) | 暂停、恢复或取消指定任务 |
| 路线 | [`route`](#route)、[`watch_path`](#watch_path)、[`path`](#path) | 读取标准路线、订阅路线快照或读取兼容快照 |

## 可运行导航流程

[`examples/navigate.rs`](../../examples/navigate.rs) 提交一次目标，打印 `request_id` 和接纳回执中的 `task_id`，再等待同一任务的终态：

```text
cargo run --example navigate -- --help
cargo run --example navigate -- http://<robot>:5052 3 2 0 1.57 120
```

第二条命令会实际发送导航目标。参数依次为端点、地图坐标 `x y z`（m）、`yaw`（rad）和可选等待秒数（默认120，须为正整数）。按实际地图与现场条件填写目标。示例支持 `DOSO_API_KEY` 和 `DOSO_TLS_CA_FILE`，`--help` 不连接设备。

拒绝、提交状态未知、任务失败或等待超时均以非零退出码结束。保留输出中的请求和任务身份以继续查询；示例不自动重试，也不因本地等待超时取消远端任务。显式取消使用 [`cancel`](#cancel)。

当前 Rust 客户端及 LingTu 导航后端已有本地 gRPC 接线与合同验证。真实机器狗上的正式 `5052` 服务部署、地图/定位准备和导航到点闭环尚未验收；示例存在不代表现场已经具备运行条件。只读位姿/路线查看继续使用 `watch_navigation`。

## 提交目标

### go_to

提交一个地图坐标导航目标。

```text
pub async fn go_to(
    &self,
    pose: Pose,
    request_id: Option<&str>,
) -> Result<CommandReceipt>
```

| 参数 | 含义 |
| --- | --- |
| `pose.x/y/z` | `map` 中的目标位置，单位 m，三个值均须有限 |
| `pose.yaw` | 目标偏航角，单位 rad，须为有限值 |
| `request_id` | 本次提交的身份，`None` 时由 SDK 生成 ULID，`Some` 时须为非空白字符串 |

前置条件由机器人侧执行方判断，包括运行中的导航能力、活动地图、定位与规划输入、控制权限及安全状态。SDK 在发送前检查位姿数值和请求 ID。当前方法的目标参数为 `Pose`，请求的其余导航策略由机器人侧配置决定。

`accepted == Some(true)` 表示目标已接纳，此时回执必须提供非空 `task_id`，SDK 会检查这一合同。目标接纳后，继续使用该 `task_id` 查询或等待执行结果。`request_id` 标识一次命令，`task_id` 标识导航任务。

下面的示例在运行时会提交导航目标。`no_run` 示例仅参与编译校验。

```rust,no_run
use doso_sdk::{new_request_id, DosoClient, Pose};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    let request_id = new_request_id()?;
    let receipt = robot.navigation().go_to(
        Pose { x: 3.0, y: 2.0, z: 0.0, yaw: 1.57 },
        Some(&request_id),
    ).await?;
    if receipt.accepted == Some(true) {
        let task_id = receipt.task_id.as_deref().ok_or("missing task_id")?;
        let result = robot.navigation().wait(task_id, Duration::from_secs(120)).await?;
        println!("{result:?}");
    } else {
        println!("admission {:?}, reason {}", receipt.accepted, receipt.reason);
    }
    Ok(())
}
```

## 查询、订阅与等待

### status

读取导航 owner 的综合诊断快照。

```text
pub async fn status(&self) -> Result<rpc::Snapshot>
```

### task

查询指定导航任务的当前状态。

```text
pub async fn task(&self, task_id: &str) -> Result<Task>
```

### watch

订阅指定任务的状态快照与终态。

```text
pub async fn watch(&self, task_id: &str) -> Result<tonic::Streaming<Task>>
```

### wait

等待同一导航任务进入终态。

```text
pub async fn wait(&self, task_id: &str, timeout: Duration) -> Result<Task>
```

| 方法 | 参数与返回 |
| --- | --- |
| `status` | 返回导航综合诊断 `Snapshot.fields`，具体字段随 owner 的状态投影提供 |
| `task` | `task_id` 须为非空白字符串，返回该身份的 `Task`，SDK 检查回传身份与已知状态值 |
| `watch` | `task_id` 同上，返回原始任务消息流，通过 `message().await` 接收样本 |
| `wait` | `timeout` 须大于零，消费 `watch`，逐条检查任务身份与状态，成功终态返回 `Task` |

`Task` 包含 `task_id`、`found`、`state`、`reason`、`reason_code` 与扩展 `details`。`found=false` 表示当前查询没有取得该任务，合法 `TaskUnknown` 表示尚无明确状态。两者在 `wait` 中继续等待，受总超时约束。

当前任务流以 50 Hz 目标发送状态快照，状态未变化时可以重复。到达 `TaskSuccess`、`TaskFailed` 或 `TaskCancelled` 后发送终态并结束。丢弃流会释放本地订阅和服务端名额，远端导航任务仍由其 owner 管理。流名额与其他 SDK 长连接共享，达到上限时返回 `RESOURCE_EXHAUSTED`。

`task` 可正常返回失败或取消状态。`wait` 对 `TaskSuccess` 返回成功，对失败或取消终态返回 `ErrorKind::OperationFailed`，完整消息保留在 `error.terminal` 的 `Terminal::Navigation` 中。等待超时只结束本地等待，后续仍可用原 `task_id` 查询或显式取消。状态枚举与统一错误语义见公共合同。[返回码与状态](../results.md) · [消息清单](../message-catalog.md)

## 暂停、恢复与取消

### pause

请求暂停指定导航任务。

```text
pub async fn pause(
    &self, task_id: &str, reason: &str, request_id: Option<&str>,
) -> Result<CommandReceipt>
```

### resume

请求恢复同一暂停任务。

```text
pub async fn resume(
    &self, task_id: &str, reason: &str, request_id: Option<&str>,
) -> Result<CommandReceipt>
```

### cancel

请求取消指定导航任务。

```text
pub async fn cancel(
    &self, task_id: &str, reason: &str, request_id: Option<&str>,
) -> Result<CommandReceipt>
```

`task_id` 为已取得的任务身份，`reason` 为非空白原因，`request_id` 标识本次控制请求。`pause`、`resume` 要求显式请求 ID 与任务 ID 不同，`None` 会生成新 ID。SDK 检查回执中的任务关联，暂停与恢复的接纳回执必须指向请求的任务。

| 方法 | 机器人侧条件与后续观察 |
| --- | --- |
| `pause` | 任务须可暂停，导航 owner 执行其停止与状态转换流程，接纳后观察 `TaskPaused` |
| `resume` | 恢复同一暂停任务，重新检查控制、安全、地图与定位等运行条件，继续观察该任务 |
| `cancel` | 请求终止指定任务，接纳后继续观察 `TaskCancelled` 等最终状态 |

```rust,no_run
use doso_sdk::DosoClient;

async fn pause_and_inspect(
    robot: &DosoClient,
    task_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let receipt = robot.navigation().pause(task_id, "operator_pause", None).await?;
    println!("pause admission {:?}", receipt.accepted);
    let task = robot.navigation().task(task_id).await?;
    println!("task state {}", task.state);
    Ok(())
}

async fn request_resume(robot: &DosoClient, task_id: &str) -> doso_sdk::Result<()> {
    let receipt = robot.navigation().resume(task_id, "operator_resume", None).await?;
    println!("resume admission {:?}", receipt.accepted);
    Ok(())
}

async fn request_cancel(robot: &DosoClient, task_id: &str) -> doso_sdk::Result<()> {
    let receipt = robot.navigation().cancel(task_id, "operator_cancel", None).await?;
    println!("cancel admission {:?}", receipt.accepted);
    Ok(())
}
```

## 路线与兼容快照

### route

读取标准 XYZ 路线缓存。

```text
pub async fn route(&self) -> Result<rpc::NavigationPath>
```

### watch_path

订阅标准路线缓存快照。

```text
pub async fn watch_path(&self) -> Result<tonic::Streaming<rpc::NavigationPath>>
```

### path

读取既有兼容路线快照。

```text
pub async fn path(&self) -> Result<rpc::Snapshot>
```

`route` 返回当前路线缓存，`watch_path` 以目标最大 100 Hz 重复读取同一缓存。`NavigationPath.points` 为 `frame_id` 中的 XYZ 点，单位 m。当前已接来源要求地图坐标系，空点集可表达路线清空。兼容上游可能为缺少高度的数据填入零，显示时应结合来源解释 z。源未提供采样时间时，`time` 保持缺失。

`path` 保留既有 `Snapshot.fields` 路线投影。需要标准点集时可使用 `route`，需要持续显示时可使用 `watch_path`。路线快照频率、任务状态快照频率、SLAM 源更新频率分别表达各自来源。

```rust,no_run
use doso_sdk::DosoClient;

async fn read_route(robot: &DosoClient) -> Result<(), Box<dyn std::error::Error>> {
    let route = robot.navigation().route().await?;
    println!("{} points in {}", route.points.len(), route.frame_id);
    let mut updates = robot.navigation().watch_path().await?;
    if let Some(route) = updates.message().await? {
        for point in route.points {
            println!("{} {} {} m", point.x, point.y, point.z);
        }
    }
    Ok(())
}
```

## 常见失败

| 情况 | 调用方处理 |
| --- | --- |
| 位姿非有限值、空任务 ID 或空原因 | SDK 本地返回 `INVALID_ARGUMENT`，修正参数 |
| 地图、定位、控制或任务当前状态不满足要求 | 检查 owner 返回的标准码、`reason_code` 与 `details`，刷新相应状态 |
| 命令回执丢失或超时 | 保留 `request_id`、已取得的 `task_id` 与 `submission_unknown`，先确认现有执行状态 |
| `wait` 总超时 | 使用同一任务 ID 继续查询，或根据业务意图发起取消 |
| 未知枚举、回执身份错配、终态前流结束 | SDK 报协议错误，保留诊断与原任务身份 |

具体标准码的分类依据为执行方证据，完整规则由返回合同维护。[返回码与状态](../results.md)
