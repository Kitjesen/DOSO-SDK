# DOSO Rust SDK API 手册

适用于当前 `doso-sdk 0.2.0-alpha.1`。开发者通过一个 `DosoClient` 连接机器人，使用七个领域完成系统查询、运动操控、定位、地图、导航、观测和高层任务。

文档核对日期为2026-11-07；2026-11-11 补充地理位置查询与订阅，2026-11-12 收紧公开系统信息并将 `product_status()` 改为类型化结果。线协议方法不变，开发预览调用方需更新返回字段读取。其他接口沿用各自验证记录。

本手册采用厂商 SDK 常用的接口说明结构，逐项列出函数签名、参数、返回、使用条件、失败行为和调用示例。连接、基础操控及运动能力的对照依据为智身 GEN2 和宇树 Go2 SDK2 的官方固定提交。[厂商对照与来源](../architecture.md#连接与操控对照)

## 文档导航

| API文档 | 开发者可以完成的操作 | RPC定义 | Rust实现 |
| --- | --- | --- | --- |
| [连接与公共约定](client.md) | 配置地址与凭据、握手、访问领域、生成请求 ID、结束连接资源 | 握手使用[system.proto](../../proto/system.proto) | [client/](../../src/client/mod.rs) |
| [system](system.md) | 读取身份、能力、健康、故障、就绪、会话和运行模式 | [system.proto](../../proto/system.proto) | [system/](../../src/system/mod.rs) |
| [motion](motion.md) | 控制权、电机、Profile、外部运控、站坐、速度控制、停止与恢复 | [motion.proto](../../proto/motion.proto) | [motion/](../../src/motion/mod.rs) |
| [localization](localization.md) | 读取地图位姿、GNSS 地理位置与定位状态，提交重定位 | [localization.proto](../../proto/localization.proto) | [localization/](../../src/localization/mod.rs) |
| [maps](maps.md) | 建图、保存、加载地图，查询结果、读取 PCD 和栅格 | [maps.proto](../../proto/maps.proto) | [maps/](../../src/maps/mod.rs) |
| [navigation](navigation.md) | 提交目标、跟踪任务、暂停、恢复、取消和读取路线 | [navigation.proto](../../proto/navigation.proto) | [navigation/](../../src/navigation/mod.rs) |
| [observations](observations.md) | 读取 IMU、关节、电机、相机、实时点云及场景 | [observations.proto](../../proto/observations.proto) | [observations/](../../src/observations/mod.rs) |
| [tasks](tasks.md) | 提交高层指令，查询、订阅和取消当前运行 | [tasks.proto](../../proto/tasks.proto) | [tasks/](../../src/tasks/mod.rs) |

构建、Cargo 接入和运行示例见快速入门。返回码和消息字段使用独立的公共说明，领域页面补充各接口的具体语义。[快速入门](../usage.md) · [返回码与状态](../results.md) · [消息格式](../message-catalog.md)

API文档说明怎么调用，`proto/*.proto`定义网络RPC，`proto/messages/`定义消息字段，`src/`实现Rust调用行为。[build.rs](../../build.rs)从协议生成消息与客户端，[lib.rs](../../src/lib.rs)负责公共导出。生成代码不单独维护。机器人端的职责、进程及跨仓库代码位置见[架构说明](../architecture.md#外部运控的代码对应)。

## 公共调用约定

### 异步运行时与返回类型

异步方法需要 Tokio 运行时，返回 `doso_sdk::Result<T>`。同步方法包括配置构造、领域访问、初次身份快照，以及遥操作本地入队和结果解码。

| 结果类型 | 调用方应检查的内容 |
| --- | --- |
| `Result<T>` | RPC 或本地检查是否成功，错误通过 `Error` 提供 |
| `CommandReceipt` | `accepted`、请求身份、所属任务或操作身份，以及 `reason/reason_code/stage` |
| `Task`、`MapOperation`、`ProductOperation` | 对应 owner 报告的当前执行状态 |
| `tonic::Streaming<T>` | 建流结果及每次 `message().await` 的结果，正常结束返回 `None` |
| `Snapshot` | `fields` 中的 Protobuf 诊断结构，具体内容取决于对应服务 |
| `ProductStatus` | 本地当前运行模式、可用性及地图切换所需会话校验值；不包含云管理配置或进程证据 |

`CommandReceipt.accepted` 是 `Option<bool>`。`Some(true)` 为明确接纳，`Some(false)` 为明确拒绝，`None` 表示源端没有提供接纳事实。动作完成由身体状态或所属任务、操作的终态报告。完整返回合同由公共状态文档维护。[返回合同](../results.md)

### 请求身份

写操作普遍接受 `request_id: Option<&str>`。传 `None` 时 SDK 生成 ULID，调用方也可以先调用 `new_request_id()` 保存身份，再提交操作。同一个操作的结果核查沿用该 ID，每次新的业务操作使用新的 ID。

`task_id` 表示导航任务，`operation_id` 表示地图保存事务，`run_id` 表示高层指令运行，`product_session_id` 表示当前产品会话。它们各自标识不同对象，参数名称与回执字段保持一致。

SDK保留写请求身份，发送失败或应答丢失时通过 `submission_unknown` 表达可能已提交。调用方应先核查源状态和原操作结果。请求错误处理与重试边界见公共状态说明。[未知提交处理](../results.md)

### Error访问

```text
pub fn Error::code(&self) -> Code
pub fn Error::reason_code(&self) -> Option<&str>
pub fn Error::details(&self) -> Option<&prost_types::Struct>
```

`code()`提供标准gRPC调用码，`reason_code()`提供owner返回的机器原因，`details()`提供原始结构化详情。错误还公开`kind/message/request_id/submission_unknown/terminal`字段。等待导航、地图或产品操作进入失败终态时，`terminal`保留所属领域的完整结果。字段含义和错误分类集中在公共状态文档中维护。[Error与终态](../results.md)

### 超时与订阅生命周期

| 调用 | 当前等待规则 |
| --- | --- |
| 普通单次 RPC | `Config.timeout`，默认10秒 |
| 站起、坐下 | `max(Config.timeout, 15秒)` |
| 重定位 | 固定60秒 |
| 导航、地图、产品操作的 `wait` | 调用方传入总等待时间 |
| 普通服务端订阅 | 建立后由调用方管理持续时间，每次接收可使用本地超时 |
| Teleop建流、等待单条ACK、hold和close | 使用 `Config.timeout` |
| ExternalControl建流、使能、禁用和close | 使用 `Config.timeout`，帧发布为同步入队 |

退出本地等待后，导航、地图保存或产品切换仍由机器人端 owner 管理。撤销操作使用所属领域提供的取消方法。丢弃普通观测流结束订阅，丢弃遥操作会话会关闭连接并触发机器人端清理。需要核对释放结果时调用 `Teleop::close()`。

每个 Gateway 的长流及地图读取共用最多8个名额，超额返回 `RESOURCE_EXHAUSTED`。取消或结束流会释放名额。IMU、关节、身体状态、位姿、路线、任务、相机、点云、运动反馈、Teleop及ExternalControl都计入该配额。

### 坐标系、单位与未知字段

| 数据 | 表达约定 |
| --- | --- |
| 平面速度命令 | 机体系XY线速度，单位m/s，Z轴角速度单位rad/s |
| 导航目标和定位位姿 | `map`坐标系，位置m，yaw为rad |
| 速度反馈 | `frame_id`指定速度表达坐标系，当前为`body` |
| 里程计反馈 | 位姿表达子坐标系在`parent_frame`中的变换，速度表达于`child_frame`，当前为`map → body` |
| 关节反馈 | 位置rad、速度rad/s、力矩N·m |
| Protobuf四元数 | 按具名`w/x/y/z`字段读取，方向由对应消息说明 |

可缺省字段映射为 Rust `Option<T>`。源端缺少有效值时保留缺失。`received_age_ms` 表示 Gateway 接收后的年龄，`SampleTime` 表达源采样时钟信息，具体可用性见对应领域页面。[消息与时间语义](../message-catalog.md)

## 推送频率

| 通道 | 当前合同 | 来源与测量边界 |
| --- | --- | --- |
| Teleop速度发布 | 调用方持续提交新输入，本地50Hz已验证 | 提交入队与机器人接纳分别返回，源窗口固定350ms有效 |
| 运动状态快照 | 默认目标最高100Hz，可选1–100Hz | 状态评估与缓存查询 |
| 定位位姿、导航路线、速度反馈 | 目标最高100Hz缓存快照 | 可重复当前值，源更新率由对应owner决定 |
| 里程计反馈 | 10Hz，每100ms一次 | 当前定位源默认状态导出约10Hz |
| 导航与高层任务 | 目标50Hz快照 | 内容可重复，终态处理见领域页面 |
| 控制权与故障 | 目标50Hz快照 | 可合并或重复当前状态，真实来源刷新率单独决定 |
| IMU与关节 | 按源事件交付，实机目标100Hz待验收 | 当前SDK没有设置硬件采样率的接口 |
| 身体状态 | 初值与源变化事件 | 事件驱动 |
| 相机 | 服务发布的JPEG帧 | 帧率取决于设备和媒体服务 |
| 显示点云 | 地图显示约最高4Hz，扫描显示约最高10Hz | 使用已有显示采样和量化格式 |

实际平均速率、帧间抖动和机器人端验证分别记录。当前定位速度反馈与里程计共用约10Hz的默认源导出，高频缓存流会重复源样本。[验证记录](../grpc-v1-progress.md)

## 当前能力缺口

控制权申请与释放、外部运控模式、状态快照、普通运动恢复及已知来源故障已接通。外部控制器由客户自行部署，通过逐关节目标接入；调用顺序及退出语义见[motion](motion.md#自定义控制器)。逐次失权事件、姿态与步态、恢复与阻尼、完整故障来源及BMS、完整任务封装和发布验收继续按实际机器人能力推进。[缺口与推进顺序](../v1-plan.md#对标后的能力缺口)
