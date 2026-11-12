# maps 地图与建图事务

通过 `client.maps()` 访问地图列表、保存任务、Product 切换事务和已保存地图内容。地图保存由现有 SLAM 与 mapd 完成，加载和开始建图通过 ProductControl 执行。

本页 `Result<T>` 为 `doso_sdk::Result<T>`，`rpc` 为 `doso_sdk::rpc`。地图保存使用 `operation_id`，Product 切换使用 `request_id`，当前运行的 Product 由 `product_session_id` 标识。[Rust 实现](../../src/maps/mod.rs) · [RPC](../../proto/maps.proto) · [消息定义](../../proto/messages/maps.proto) · [返回合同](../results.md)

## 方法目录

| 分组 | 方法 | 用途 |
| --- | --- | --- |
| 地图目录 | [`list`](#list) | 查询已保存地图与当前活动地图 |
| 保存事务 | [`save`](#save)、[`operation`](#operation)、[`wait`](#wait) | 提交保存，查询或等待同一操作 |
| Product 事务 | [`load`](#load)、[`start_mapping`](#start_mapping)、[`product_operation`](#product_operation)、[`wait_product_operation`](#wait_product_operation) | 加载保存地图进入导航，或开始新建图 |
| 地图内容 | [`grid`](#grid)、[`point_cloud`](#point_cloud) | 读取保存栅格或分块读取 PCD 文件 |

## 地图列表

### list

读取已保存地图目录和活动地图。

```text
pub async fn list(&self) -> Result<rpc::MapList>
```

`MapList.maps` 中每个 `MapRecord` 含 `name`、`has_pcd`、`has_octomap`、可选 `can_activate` 和 `activation_blockers`。`can_activate=None` 表示来源未明确给出可激活结论。`MapList.active` 为活动地图名，空字符串表示该返回未给出活动地图。`details` 保留 owner 的目录投影。

目录中的内容标志与激活条件分别读取。地图文件存在后，加载条件仍由 ProductControl 与相应服务确认。普通地图 ID 的公开规则为最长 100 字符，使用字母、数字、下划线、点和短横线，起始字符及保留名还受服务校验。SDK 对所需名称先检查非空白，完整合法性与资源可用性由服务端判断。

## 保存、查询与等待

### save

提交一次地图保存操作。

```text
pub async fn save(
    &self, name: Option<&str>, request_id: Option<&str>,
) -> Result<CommandReceipt>
```

### operation

查询指定地图保存操作。

```text
pub async fn operation(&self, operation_id: &str) -> Result<MapOperation>
```

### wait

轮询等待同一地图保存操作进入终态。

```text
pub async fn wait(
    &self,
    operation_id: &str,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<MapOperation>
```

| 参数 | 含义 |
| --- | --- |
| `name` | `Some` 指定保存名称且须非空白，`None` 由当前 Gateway 生成名称 |
| `request_id` | 本次保存请求的身份，`None` 由 SDK 生成 ULID，`Some` 须非空白 |
| `operation_id` | 保存执行方返回的操作 ID，查询与等待须沿用该 ID |
| `timeout` | 本地等待总时限，须大于零 |
| `poll_interval` | 保存状态轮询间隔，须大于零，使用 `Duration` 表达 |

前置条件包括当前 SLAM 后端支持保存、存在可保存来源、mapd 可用，以及当前产品对保存时机的约束。`save` 返回接纳回执，`accepted == Some(true)` 时必须附带非空 `operation_id`，SDK 会检查这一合同。

`operation` 返回指定保存任务的当前 `MapOperation`。等待快照、排队、执行中和合法未知状态会继续等待。`wait` 遇到 `MapSucceeded` 返回成功，失败或取消终态返回附带 `Terminal::Map` 的执行错误。总超时只结束本地轮询，保存操作继续由原 owner 管理。保存持久化完成由操作终态确认。

以下示例运行时会保存当前地图。

```rust,no_run
use doso_sdk::{new_request_id, DosoClient};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    let request_id = new_request_id()?;
    let receipt = robot.maps().save(Some("lab"), Some(&request_id)).await?;
    if receipt.accepted == Some(true) {
        let operation_id = receipt.operation_id.as_deref().ok_or("missing operation_id")?;
        let saved = robot.maps().wait(
            operation_id, Duration::from_secs(120), Duration::from_millis(200),
        ).await?;
        println!("saved operation {}", saved.operation_id);
    } else {
        println!("admission {:?}, reason {}", receipt.accepted, receipt.reason);
    }
    Ok(())
}
```

## 加载地图与开始建图

### load

请求加载保存地图并切换到导航 Product。

```text
pub async fn load(
    &self,
    map_name: &str,
    expected_product_session_id: &str,
    initial_pose: Option<Pose>,
    request_id: Option<&str>,
) -> Result<ProductOperation>
```

### start_mapping

请求开始新建图 Product 会话。

```text
pub async fn start_mapping(
    &self,
    expected_product_session_id: &str,
    request_id: Option<&str>,
) -> Result<ProductOperation>
```

### product_operation

查询指定 Product 切换事务。

```text
pub async fn product_operation(&self, request_id: &str) -> Result<ProductOperation>
```

### wait_product_operation

等待 Product 切换完成并检查活动结果。

```text
pub async fn wait_product_operation(
    &self,
    request_id: &str,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<ProductOperation>
```

| 参数 | 含义 |
| --- | --- |
| `map_name` | 已保存的目标地图，须非空白，由执行方确认地图及激活产物可用 |
| `expected_product_session_id` | 调用前观察到的 ProductControl 当前会话身份，用于比较后切换 |
| `initial_pose` | `Some(Pose)` 提供保存地图 `map` 坐标系中的定位初值，XYZ 为 m，yaw 为 rad，均须有限 |
| `request_id` | Product 切换事务的身份，后续查询沿用该 ID，`None` 时生成 ULID |
| `timeout`、`poll_interval` | 查询等待的总时限与间隔，均须大于零 |

调用前通过 `system().product_status()` 返回的 `ProductStatus.product_session_id` 读取当前会话，再原样传入预期身份。执行方在修改锁内比较当前会话，状态已变化时应刷新后重新决定切换。只有已确认当前没有 Product 会话时，空字符串才表达该预期。该值供 SDK 操作一致性校验，普通客户界面无需显示。

`load` 对应切换到 `Product::Nav`，加载保存地图并执行定位准备。`initial_pose=None` 省略定位种子，具体定位流程由 ProductControl 决定。`start_mapping` 对应 `Product::Map` 的新建图会话，请求中省略保存地图和定位初值。跨会话恢复旧优化图继续建图的能力状态以产品说明为准。

两项提交都直接返回 `ProductOperation`，其 `state` 描述事务当前阶段。`ProductSucceeded` 与 `active=true` 共同确认切换的成功结果，`wait_product_operation` 会检查这项合同。失败或中断终态保留在 `Terminal::Product` 中，总超时只结束本地等待。

同一切换请求 ID 应对应同一参数。并发切换、会话身份变化、地图缺失、定位准备失败或 ProductControl 不可用会由执行方报告。`ProductOperation.details` 保留事务的原始结果，回传身份必须匹配查询请求。

```rust,no_run
use doso_sdk::{new_request_id, DosoClient, Pose, ProductOperation};
use std::time::Duration;

async fn load_saved_map(
    robot: &DosoClient,
    map_name: &str,
    observed_product_session_id: &str,
) -> doso_sdk::Result<ProductOperation> {
    let request_id = new_request_id()?;
    let operation = robot.maps().load(
        map_name,
        observed_product_session_id,
        Some(Pose { x: 1.0, y: 2.0, z: 0.0, yaw: 0.5 }),
        Some(&request_id),
    ).await?;
    robot.maps().wait_product_operation(
        &operation.request_id, Duration::from_secs(120), Duration::from_millis(200),
    ).await
}

async fn begin_new_mapping(
    robot: &DosoClient,
    observed_product_session_id: &str,
) -> doso_sdk::Result<ProductOperation> {
    robot.maps().start_mapping(observed_product_session_id, None).await
}
```

切换地图使用上述 Product 事务。`localization().relocalize(...)` 针对当前活动地图，其请求地图必须与活动地图匹配。地图保存、保存内容读取、地图激活和当前地图重定位各有独立的结果身份。[使用说明](../usage.md) · [返回码与事务状态](../results.md)

## 保存的二维栅格

### grid

读取保存地图的二维占据栅格。

```text
pub async fn grid(&self, map_name: &str) -> Result<rpc::MapGrid>
```

返回指定保存地图的占据栅格，当前来源为 mapd 的保存产物。读取后的活动 Product 与地图仍由原生命周期事务管理。

| 字段 | 语义 |
| --- | --- |
| `map_id`、`content_epoch` | 地图身份及可选内容代次 |
| `frame_id`、`origin` | 栅格坐标系与二维地图原点位姿，位置为 m，yaw 为 rad |
| `width`、`height` | 单元格数量，均须大于零 |
| `resolution_m` | 每个单元格的边长，单位 m，须为有限正值 |
| `cells` | 行优先字节数组，x 先变化，再沿 +y 换行，索引为 `y * width + x` |
| `time` | 来源明确提供的采样时间，当前保存产物未提供时保持缺失 |

占据值范围为 0–100，255 表示未知。当前 SDK 检查地图身份、坐标系、原点、数组长度和合法取值，限制为 25,000,000 个单元格。二维原点的 z 按产物语义解释，显示层保留未知单元格的含义。

```rust,no_run
use doso_sdk::DosoClient;

async fn read_saved_grid(robot: &DosoClient, map_name: &str) -> doso_sdk::Result<()> {
    let grid = robot.maps().grid(map_name).await?;
    let unknown = grid.cells.iter().filter(|cell| **cell == 255).count();
    println!("{} {}x{}, {} m/cell, {} unknown", grid.map_id,
        grid.width, grid.height, grid.resolution_m, unknown);
    Ok(())
}
```

## 保存的 PCD 点云文件

### point_cloud

分块读取保存地图的 PCD 文件。

```text
pub async fn point_cloud(
    &self, map_name: &str,
) -> Result<tonic::Streaming<rpc::MapContent>>
```

返回一次文件下载流。首条 `MapContent.value` 为 `Info`，包含 `map_id`、`size_bytes`、可选 `content_epoch` 和 `frame_id`。后续消息为 `Chunk`，按顺序拼接得到 PCD 文件。首条元数据身份、消息顺序、流正常结束及累计字节数共同构成完整读取条件，流类型由协议定义。[点云消息](../../proto/messages/maps.proto) · [Rust 下载示例](../../examples/download_map.rs)

```rust,no_run
use doso_sdk::{rpc::map_content::Value, DosoClient};

async fn count_saved_pcd(
    robot: &DosoClient,
    map_name: &str,
) -> Result<u64, Box<dyn std::error::Error>> {
    let mut stream = robot.maps().point_cloud(map_name).await?;
    let info = match stream.message().await?.and_then(|message| message.value) {
        Some(Value::Info(info)) if info.map_id == map_name => info,
        _ => return Err("missing or mismatched map metadata".into()),
    };
    let mut received = 0_u64;
    while let Some(message) = stream.message().await? {
        let Some(Value::Chunk(chunk)) = message.value else {
            return Err("unexpected map content message".into());
        };
        received = received.checked_add(chunk.len() as u64).ok_or("size overflow")?;
        if received > info.size_bytes {
            return Err("map exceeds declared size".into());
        }
    }
    if received != info.size_bytes {
        return Err("incomplete map download".into());
    }
    Ok(received)
}
```

文件内容读取和实时点云预览使用各自的协议。实时显示帧来自 `observations().watch_point_cloud(...)`，其显示采样、坐标和代次语义见消息清单。PCD 下载占用共享流名额，丢弃流会终止该次下载并释放资源。[消息清单](../message-catalog.md)

## 常见失败

| 情况 | 调用方处理 |
| --- | --- |
| 空白名称、操作 ID 或零等待时限 | 修正参数，SDK 本地报告 `INVALID_ARGUMENT` |
| 当前 SLAM 不支持保存，或保存来源未就绪 | 读取 owner 的 `reason_code` 和当前保存状态 |
| 地图、产物或操作已确认不存在 | 按 `NOT_FOUND` 与具体原因处理 |
| 内容正在写入、Product 会话变化或并发切换 | 刷新目录或 ProductControl 状态，再决定下一次操作 |
| mapd、ProductControl 或传输不可用 | 保留请求与操作身份，按 `UNAVAILABLE` 和未知提交信息继续确认 |
| 栅格结构非法、下载内容损坏或身份错配 | 保留协议或数据错误，完成条件满足后再使用完整内容 |

`reason_code` 和标准码均以实际执行方结果为准。保存失败、切换失败以及本地等待结束的区别由公共合同统一解释。[返回码与状态](../results.md)
