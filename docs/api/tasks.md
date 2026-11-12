# tasks 高层任务

通过 `client.tasks()` 向现有任务 owner 提交自然语言指令，查询、订阅或取消当前运行任务。指令可能触发导航等机器人操作，执行能力和控制分类由机器人侧任务 owner 提供。

本页 `Result<T>` 为 `doso_sdk::Result<T>`，`rpc` 为 `doso_sdk::rpc`。`run_id` 标识高层任务运行，导航子任务使用自己的 `task_id`。请求身份、Run 状态和机器人运动状态分别保留。[Rust 实现](../../src/tasks/mod.rs) · [状态解析](../../src/tasks/state.rs) · [订阅](../../src/tasks/watch.rs) · [RPC](../../proto/tasks.proto) · [消息](../../proto/messages/tasks.proto)

## 方法目录

| 方法 | 用途 |
| --- | --- |
| [`send_instruction`](#send_instruction) | 提交普通任务指令或仅控制指令 |
| [`get_run`](#get_run) | 读取 owner 的当前 Run 原始快照 |
| [`get_run_state`](#get_run_state) | 查询 Run 并解析为可选 `TaskRunState` |
| [`watch_run`](#watch_run) | 订阅指定 Run 的重复快照与终态 |
| [`cancel_run`](#cancel_run) | 请求取消指定 Run |
| [`rpc::Run::state`](#run-state) | 从查询或流消息中解析任务状态、结果摘要与原因 |

## 提交指令

### send_instruction

提交普通任务指令或仅控制指令。

```text
pub async fn send_instruction(
    &self,
    instruction: &str,
    run_id: Option<&str>,
    control_only: bool,
    request_id: Option<&str>,
) -> Result<CommandReceipt>
```

| 参数 | 含义 |
| --- | --- |
| `instruction` | 指令文本，SDK 检查非空白，当前 Gateway 长度上限为 1024 字符 |
| `run_id` | 可选运行身份，`Some` 时须非空白，控制路径将其传给现有任务 owner |
| `control_only` | `true` 时仅处理 owner 能识别的控制指令，普通启动指令会被拒绝，`false` 时使用正常指令入口 |
| `request_id` | 本次命令身份，`None` 生成 ULID，`Some` 须为非空白字符串 |

普通指令经过运动权限和安全条件检查。任务控制指令的识别规则由 owner 提供，SDK 保留文本原意。`control_only` 表达调用者对指令类型的限制，安全与控制判断仍由机器人侧执行。

回执 `accepted == Some(true)` 表示指令接纳。新任务的运行身份以返回的 `receipt.run_id` 为准，当前 AgentPlanner 创建任务时自行生成运行 ID。调用时提供的 `run_id` 主要用于任务控制路径。

完整 Run 查询能力要求当前 Host 装配 AgentPlanner。仅装配 instruction 输出端口的 Host 也可能接纳文本，此时回执可缺少可追踪的 `run_id`。调用方根据实际回执选择后续查询。

以下示例运行时会提交一条任务指令，并为有 Run 身份的任务观察最多 60 秒。

```rust,no_run
use doso_sdk::{DosoClient, TaskRunStatus};
use std::time::Duration;

async fn observe_run(
    robot: &DosoClient,
    run_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut updates = robot.tasks().watch_run(run_id).await?;
    while let Some(run) = updates.message().await? {
        let Some(state) = run.state()? else {
            println!("run {} is not currently retained", run.run_id);
            break;
        };
        println!("run {} status {}, result {}", state.run_id, state.status, state.result);
        if matches!(TaskRunStatus::try_from(state.status)?,
            TaskRunStatus::Succeeded | TaskRunStatus::Failed | TaskRunStatus::Cancelled)
        {
            break;
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    let receipt = robot.tasks().send_instruction("检查门口", None, false, None).await?;
    if receipt.accepted == Some(true) {
        if let Some(run_id) = receipt.run_id.as_deref() {
            tokio::time::timeout(Duration::from_secs(60), observe_run(&robot, run_id)).await??;
        }
    } else {
        println!("admission {:?}, reason {}", receipt.accepted, receipt.reason);
    }
    Ok(())
}
```

## 查询 Run

### get_run

读取指定 Run 的当前 owner 快照。

```text
pub async fn get_run(&self, run_id: &str) -> Result<rpc::Run>
```

### get_run_state

查询 Run 并解析为可选的类型化任务状态。

```text
pub async fn get_run_state(&self, run_id: &str) -> Result<Option<rpc::TaskRunState>>
```

`run_id` 须为非空白。`get_run` 返回 `Run { run_id, found, details }`，SDK 检查回传运行身份。`get_run_state` 在查询后调用 `Run::state()`，返回类型化状态或 `None`。

当前 owner 保留当前 Host Run。Agent 未装配或请求的 Run 已经不是当前 Run 时，查询返回 `found=false`，状态解析得到 `None`。`details` 可提供 `agent_unavailable`、`not_current` 等原因。调用方据此表达当前可查询性，历史任务保留能力由任务 owner 的后续接口决定。

```rust,no_run
use doso_sdk::DosoClient;

async fn read_run(robot: &DosoClient, run_id: &str) -> doso_sdk::Result<()> {
    let raw = robot.tasks().get_run(run_id).await?;
    println!("found {}", raw.found);
    let state_from_message = raw.state()?;
    let state_from_query = robot.tasks().get_run_state(run_id).await?;
    println!("{state_from_message:?}");
    println!("{state_from_query:?}");
    Ok(())
}
```

## 解析 Run 状态

### Run-state

`rpc::Run::state()` 将一条 Run 消息解析为可选任务状态。

```text
impl rpc::Run {
    pub fn state(&self) -> Result<Option<rpc::TaskRunState>>
}
```

该方法可用于 `get_run` 的返回值和 `watch_run` 的每条消息，按当前 owner 的 `details.state` 解析。

| owner 状态 | `TaskRunStatus` |
| --- | --- |
| `running` | `Running` |
| `done` | `Succeeded` |
| `failed` | `Failed` |
| `cancelled` | `Cancelled` |

解析结果保留原 `run_id`，`details.failure_reason` 投影到 `reason`，`details.summary` 投影到 `result`。当前来源未提供统一 `task_type`、`phase`、`reason_code`、`progress` 或源采样时间，这些字段保留空字符串或 `None`。协议中声明的其他枚举值以实际 owner 的来源接线为准。

`found=false` 得到 `Ok(None)`。`found=true` 时缺少 `details`、状态字符串不受支持，或应为字符串的状态、摘要、原因字段返回其他类型，会得到协议错误。`Succeeded` 表示任务 owner 报告完成，导航子任务与身体运动证据继续由对应领域接口提供。

## 订阅 Run

### watch_run

订阅当前 Run 的状态快照与终态。

```text
pub async fn watch_run(
    &self, run_id: &str,
) -> Result<tonic::Streaming<rpc::Run>>
```

`run_id` 须为非空白。当前 Gateway 以 50 Hz 目标发送当前 Run 快照，状态未变时可以重复。收到 owner 的 `done`、`failed`、`cancelled` 终态，或 `found=false` 后，服务发送该条消息并结束流。

流使用共享订阅名额，满额时返回 `RESOURCE_EXHAUSTED`。丢弃订阅会释放资源，任务本身继续由 owner 执行。调用方可像上面的示例一样使用 `tokio::time::timeout` 限定本地观察时长，超时后的取消行为由调用方另外发出。

50 Hz 表达任务状态快照的目标发送节拍。SLAM 状态默认约 10 Hz 的源更新、运动学缓存、任务状态变化和任务执行进度分别统计，连续收到相同 Run 表示同一运行状态仍可观察。[消息清单](../message-catalog.md)

## 取消 Run

### cancel_run

请求取消指定 Run。

```text
pub async fn cancel_run(
    &self, run_id: &str, request_id: Option<&str>,
) -> Result<CommandReceipt>
```

`run_id` 须为非空白，`request_id` 标识此次取消请求。机器人侧任务 owner 处理取消，SDK 检查回执中出现的 `run_id` 与请求一致。取消接纳后，通过 `get_run_state` 或已有流确认 `Cancelled` 等后续状态。

```rust,no_run
use doso_sdk::DosoClient;

async fn request_run_cancel(robot: &DosoClient, run_id: &str) -> doso_sdk::Result<()> {
    let receipt = robot.tasks().cancel_run(run_id, None).await?;
    println!("cancel admission {:?}, reason {}", receipt.accepted, receipt.reason);
    if let Some(state) = robot.tasks().get_run_state(run_id).await? {
        println!("current owner state {}", state.status);
    }
    Ok(())
}
```

## 常见失败

| 情况 | 调用方处理 |
| --- | --- |
| 空白文本、Run ID 或请求 ID | 修正参数，SDK 本地返回 `INVALID_ARGUMENT` |
| `control_only=true` 的文本被识别为普通启动指令 | 检查控制意图，读取 owner 给出的拒绝原因 |
| 普通任务的运动、安全或控制前置条件未满足 | 读取标准码、稳定原因与阻塞详情 |
| Agent 缺失或 Run 已不再是当前 Run | 根据 `found=false` 或 `None` 表达当前不可查询 |
| 取消被 owner 拒绝 | 保留取消回执的原因，继续确认原 Run 的状态 |
| 写入回执超时或不可用 | 保留请求 ID 和 `submission_unknown`，确认已有运行，SDK 保持显式写入行为 |
| Run 身份错配或状态格式非法 | 按协议错误处理并保留原始诊断 |

命令接纳、owner 完成、RPC 错误与未知提交分别解释，完整细节由公共合同维护。[返回码与状态](../results.md)

## 巡检路线与任务

巡检通过 `tasks()` 访问，复用 LingTu 已有巡检路线、原生 `nav.inspection`、任务时间线与证据录制事务。SDK 调用不会创建另一套巡检执行器，也不经过 HTTP 网络转发。[Rust实现](../../src/tasks/inspection.rs) · [消息](../../proto/messages/inspection.proto)

### 路线方法

```text
inspection_routes(&self, map_id: &str) -> Result<rpc::InspectionRoutes>
inspection_route(&self, map_id: &str, route_id: &str) -> Result<rpc::InspectionRoute>
put_inspection_route(&self, route: prost_types::Struct, request_id: Option<&str>)
    -> Result<rpc::InspectionRoute>
delete_inspection_route(&self, map_id: &str, route_id: &str, request_id: Option<&str>)
    -> Result<CommandReceipt>
```

以上均为 `pub async fn`。地图与路线ID须非空白。读取结果包含类型化 `map_id/route_id/revision`，`details` 沿用当前 `lingtu.inspection.v1` 路线字段。新增或更新成功后，`request_id` 对应此次请求；SDK 检查返回的地图、路线与请求身份。路线存储依赖已有 mapd 记录，地图内容版本冲突沿用 owner 的拒绝结果。

`put_inspection_route` 的 `Struct` 字段如下；字段与 HTTP 巡检配置共用同一 Pydantic 校验，不单独维护一份路线模型。

| 字段 | 含义 |
| --- | --- |
| `id/map_id` | 路线与地图ID，必填 |
| `revision` | 必填，正整数；更新使用当前 owner 的路线修订规则 |
| `map_content_epoch` | 可选地图内容版本，服务端核对实际记录 |
| `name` | 可选路线名 |
| `points` | 至少一个点；每点含 `id/x/y/z/yaw/tolerance/dwell/action/enabled` |
| `loop_count/loop_interval_s` | 循环次数与间隔，默认1次、0秒 |
| `failure_policy/max_retries` | 失败处理与重试次数，默认 `stop`、0次 |

位置单位m，yaw为rad，tolerance为正数，dwell为秒；证据动作必须是已配置 worker 支持的动作。`Struct` 数字使用双精度，超过精确整数范围的路线修订值可用十进制字符串提交，类型化响应的 `revision` 保留 `u64`。路线写入的请求ID用于关联本次调用；它不额外提供原生路线存储尚未实现的请求日志或自动重试。

### 执行与控制

```text
start_inspection(&self, map_id: &str, route_id: &str, revision: u64,
    binding: rpc::InspectionBinding, request_id: Option<&str>) -> Result<CommandReceipt>
inspection_task(&self, task_id: &str) -> Result<rpc::InspectionTask>
inspection_tasks(&self, map_id: Option<&str>, route_id: Option<&str>,
    include_terminal: bool, limit: Option<u32>) -> Result<rpc::InspectionTasks>
watch_inspection(&self, task_id: &str) -> Result<tonic::Streaming<rpc::InspectionTask>>
pause_inspection(&self, task_id: &str, reason: &str,
    binding: rpc::InspectionBinding, request_id: Option<&str>) -> Result<CommandReceipt>
resume_inspection(&self, task_id: &str, reason: &str,
    binding: rpc::InspectionBinding, request_id: Option<&str>) -> Result<CommandReceipt>
cancel_inspection(&self, task_id: &str, reason: &str,
    binding: rpc::InspectionBinding, request_id: Option<&str>) -> Result<CommandReceipt>
inspection_report(&self, task_id: &str) -> Result<rpc::Snapshot>
```

以上均为 `pub async fn`。`InspectionBinding.product_session_id` 必须是当前 Product 会话，来自 `system().product_status()`；`env` 可填 `real/sim`，空字符串表示按会话校验。启动的地图必须是当前活动地图，`revision=0` 选择当前存储版本，非零时要求完全一致。控制操作也要求绑定会话仍然一致。只读查询无需绑定。

`inspection_tasks` 列出当前巡检 owner 留存的任务投影，地图/路线过滤传 `None` 表示不限，传入的过滤值须非空白。`include_terminal=false` 只列未结束任务，`true` 可找回仍留存的终态；`limit=None` 默认20条，0按现有 owner 钳到1，最大值按 owner 的留存容量钳定。结果保留每个任务原有ID与状态，按当前投影的留存顺序倒序返回；`retention` 为 `process_local_gateway_projection` 或 `durable_gateway_projection`，`observed_at_unix_s` 是此次列表快照时间。它不是 Agent Run 历史，也不保证保留全部历史。进程本地投影可能随重启消失，启用 journal 的投影仍受原有留存与连续性规则约束。

启动沿用现有请求日志、路线快照、证据 worker 准入与录制回滚；暂停、恢复和取消沿用同一任务事务。启动、恢复受到安全停止检查，所有运动相关请求仍受控制权检查。原生业务ACK只证明请求接纳，成功回执提供 `task_id`；SDK 不把它当成已经运行或任务完成。

`InspectionTask` 保留以下 owner 事实：

| 字段 | 含义 |
| --- | --- |
| `task_id/found` | 查询身份与时间线是否留存 |
| `current_state/state_available` | 已知生命周期状态；缺少原生事件时保持空状态与 `false` |
| `phase` | 当前原生阶段，如 `NAVIGATING/DWELLING/PAUSING` |
| `terminal/execution_confirmed` | owner 是否确认终态、是否已有执行事实 |
| `reason/details` | 原因与保留的路线身份、进度、时间线及证据录制状态 |

生命周期状态为 `PLANNING/EXECUTING/PAUSED/RECOVERING/SUCCESS/FAILED/CANCELLED`，空值表示尚无状态事实。`watch_inspection` 使用与其他长流共用的8个名额，以目标50Hz发送可重复快照，发出 `terminal=true` 或 `found=false` 后结束。调用方逐帧核对 `task_id` 并处理 `message()` 错误；丢弃或取消本地流不取消机器人任务。

`inspection_report` 复用已有业务报告，结合不可变路线快照与证据记录评估结果。原生运动成功与证据完整是不同事实；报告缺失、日志损坏、修订身份不完整或证据不可读时保留原有错误，不能以空报告冒充完成。

```rust,no_run
use doso_sdk::{DosoClient, InspectionBinding};

async fn inspect(robot: &DosoClient, session: &str) -> Result<(), Box<dyn std::error::Error>> {
    let tasks = robot.tasks();
    let receipt = tasks.start_inspection("field-map", "route-a", 1, InspectionBinding {
        product_session_id: session.into(), env: "real".into(),
    }, None).await?;
    if !receipt.ok || receipt.accepted != Some(true) {
        return Err(format!("inspection rejected: {}", receipt.reason).into());
    }
    let task_id = receipt.task_id.ok_or("missing inspection task identity")?;
    let mut updates = tasks.watch_inspection(&task_id).await?;
    while let Some(state) = updates.message().await? {
        if state.task_id != task_id { return Err("inspection identity mismatch".into()); }
        println!("{} {} {}", state.current_state, state.phase, state.reason);
        if !state.found { return Err("inspection task no longer retained".into()); }
        if state.terminal {
            return if state.current_state == "SUCCESS" {
                Ok(())
            } else {
                Err(format!("inspection ended {}: {}", state.current_state, state.reason).into())
            };
        }
    }
    Err("inspection stream ended before terminal state".into())
}
```

2026-11-13 的验证覆盖本地 Rust gRPC 调用、共享 HTTP 回归和 Python gRPC 到现有 owner 的接入；真实机器人巡检、到点证据采集与完整任务执行尚未现场验收。逐项结果见仓库测试记录。
