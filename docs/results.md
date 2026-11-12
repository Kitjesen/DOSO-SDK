# SDK 返回码与状态定义

日期：2026-11-03。范围：SDK 调用码、命令接纳和领域终态。统一错误访问、机器原因传递、接纳投影和终态保留已接入；业务错误的全量分类仍以对应 owner 的明确信息逐项完善。

## 三种事实

| 事实 | 回答的问题 | 表达方式 |
| --- | --- | --- |
| 调用结果 | 这次调用是否正常完成，失败属于哪一类？ | gRPC 标准 `Code`，具体原因 `reason_code`，说明 `message` |
| 命令接纳 | 执行方是否明确接纳了这条命令？ | `UNKNOWN`、`ACCEPTED`、`REJECTED` |
| 执行状态 | 已接纳的任务或事务现在进行到哪一步？ | 所属领域的状态枚举与任务/操作 ID |

机器人运行状态由相应服务提供：Gateway 汇总会话、控制权、定位、导航、地图和安全信息；它本身不是 SLAM、规划或硬件执行方。一次状态查询可以成功返回“定位丢失”或“机器人未就绪”，此时调用码仍为 `OK`。

## 智身参考与采用范围

核对智身 GEN2 固定提交 `e8f959a22a6e5714bc299a551b45e1c182ac05c7` 的官方文档和头文件：

- SDK 的 `std::error_code / Errc` 描述本地调用、连接、握手等错误。例如 `Success=0`、`ConnectFailed=10003`、`UnsupportedDeviceOperation=10004`，还可能返回 C++ 标准错误。[SDK 错误定义](https://github.com/zsibot/genisom_robot_sdk/blob/e8f959a22a6e5714bc299a551b45e1c182ac05c7/docs/zh/sdk_error_zh.md)
- JSON 协议另有请求结果：`SUCCESS=0`、`ACCEPTED=1`、`INVALID_STATE=1006`、`RESULT_UNKNOWN=1303` 等；异步请求的接纳和最终结果分别报告。[协议结果码](https://github.com/zsibot/genisom_robot_sdk/blob/e8f959a22a6e5714bc299a551b45e1c182ac05c7/docs/zh/protocol/Protocol-1.3.0.md#L49)
- `TaskStatus` 单独表示 `UNKNOWN/STARTING/RUNNING/SUCCESS/FAILURE/STOPPED`；本体模式、运动状态、急停及故障另有类型。控制 ACK 不能证明动作已经完成。[类型](https://github.com/zsibot/genisom_robot_sdk/blob/e8f959a22a6e5714bc299a551b45e1c182ac05c7/docs/zh/sdk_type_zh.md) · [回调](https://github.com/zsibot/genisom_robot_sdk/blob/e8f959a22a6e5714bc299a551b45e1c182ac05c7/docs/zh/sdk_callback_zh.md)

采用这种职责划分。DOSO 的调用码沿用已有 gRPC 标准；接纳、执行状态和机器人故障分别解释。智身协议结果码 `1001` 与其机器人故障码 `1001` 含义不同，不能去掉类别只保留整数。厂商控制权优先级与姿态枚举不直接移植到 DOSO。

## 调用码

数值采用 [gRPC 标准状态码](https://grpc.io/docs/guides/status-codes/)，Rust 复用 `tonic::Code`，不在 proto 再维护一份同值枚举。以下是分类定义，不表示全部分支已接线。

| 值 | 名称 | DOSO 解释 |
| ---: | --- | --- |
| 0 | `OK` | 此次调用正常完成，继续检查返回数据中的接纳或执行状态 |
| 1 | `CANCELLED` | 此次调用被取消；远端任务是否取消仍以任务状态为准 |
| 2 | `UNKNOWN` | 错误信息不足，无法可靠分类 |
| 3 | `INVALID_ARGUMENT` | 请求参数本身无效 |
| 4 | `DEADLINE_EXCEEDED` | RPC 或本地等待超时；远端写操作可能已执行 |
| 5 | `NOT_FOUND` | 执行方明确报告指定资源不存在 |
| 6 | `ALREADY_EXISTS` | 创建请求遇到已经存在的实体 |
| 7 | `PERMISSION_DENIED` | 已识别的调用者没有操作权限或被明确拒绝控制权 |
| 8 | `RESOURCE_EXHAUSTED` | 配额、容量或并发订阅名额耗尽 |
| 9 | `FAILED_PRECONDITION` | 当前模式、地图、定位或安全条件不允许操作 |
| 10 | `ABORTED` | 并发事务冲突导致操作中止 |
| 11 | `OUT_OF_RANGE` | 超出当前资源的有效范围 |
| 12 | `UNIMPLEMENTED` | 服务不支持该操作 |
| 13 | `INTERNAL` | 服务内部异常或已确认的协议不变量损坏 |
| 14 | `UNAVAILABLE` | 连接或依赖服务当前不可用 |
| 15 | `DATA_LOSS` | 已确认的数据丢失或损坏 |
| 16 | `UNAUTHENTICATED` | 缺少有效身份凭据 |

`UNKNOWN` 表示错误类别不明；“写入结果未知”是另一项事实。例如 `UNAVAILABLE` 可以同时带有 `submission_unknown=true`。收到超时或不可用码不能直接推断命令被拒绝。

本地参数检查对应 `INVALID_ARGUMENT`，连接失败对应 `UNAVAILABLE`，本地等待超时对应 `DEADLINE_EXCEEDED`。已有 gRPC 错误保留原始标准码。无法进一步分类的执行失败使用 `UNKNOWN`，同时保留其领域终态和原始原因；不通过匹配展示文字猜测具体错误类别。

Rust 通过 `Error::code()` 统一访问这套码；`ErrorKind` 继续区分本地输入、连接、RPC、协议、执行失败和等待超时。`OperationFailed` 的通用码为 `UNKNOWN`，其具体失败、取消或中断状态保存在 `error.terminal`，并未丢失。

## 命令接纳

这里定义三个语义状态，暂不新增一套与 `accepted` 并存的持久字段。

| 状态 | 含义 | 当前字段的表达 |
| --- | --- | --- |
| `ACCEPTED` | 执行方明确接纳命令 | `accepted=true` |
| `REJECTED` | 执行方明确拒绝命令 | `accepted=false`，或有明确拒绝证据的错误结果 |
| `UNKNOWN` | 接纳信息缺失，或请求可能已发送但未获得明确回执 | `accepted` 未提供，或写错误 `submission_unknown=true` |

本地校验失败、命令尚未发送时没有远端接纳状态。`ok`、`success`、`stage` 不作为新的接纳事实来源；owner 返回的这些字段必须按该接口的语义解读，不能通用地将所有布尔值合并。

当前兼容规则：有明确 `accepted` 时，`ok` 从它派生；没有 `accepted` 时，只保留顶层同步 `ok/success` 的结果含义，接纳仍未知。顶层与嵌套命令的 `accepted` 冲突时返回 `INTERNAL` 并附操作证据，避免投影成确定接纳或确定拒绝。

2026-11-12 起，未预期服务异常使用固定 `INTERNAL`、`reason_code=internal_error` 与公开错误文案，异常栈和内部文件路径仅写服务日志。已定义的业务拒绝码、请求身份、写请求的未知提交结果继续保留，不能用通用内部错误代替已有操作事实。

导航提交、保存地图和请求取消都以执行方回执确认接纳。导航完成、地图持久化完成、任务取消完成分别由后续状态确认。软件停止 ACK 不能用来证明机器人已经物理停稳。

写入回执不明时保留 `request_id` 和 `submission_unknown`。调用者先查询已有任务或操作；SDK 保持不自动重发写命令的行为。

写请求的回执超过客户端接收上限时，Tonic 也可能返回 `OUT_OF_RANGE`；此时执行方可能已接纳，因此保守标记未知提交，保留原标准码。返回码分类本身不能证明写入没有发生。

## 执行状态

保持现有协议值，状态由所属 owner 提供。每组枚举只在其领域文件中维护。

| 领域 | 当前状态及数值 | 终态 |
| --- | --- | --- |
| 导航 `TaskState` | `UNKNOWN=0, PLANNING=1, EXECUTING=2, PAUSED=3, RECOVERING=4, SUCCESS=5, FAILED=6, CANCELLED=7` | `SUCCESS / FAILED / CANCELLED` |
| 地图保存 `MapState` | `UNKNOWN=0, WAITING_SNAPSHOT=1, QUEUED=2, RUNNING=3, SUCCEEDED=4, FAILED=5, CANCELLED=6` | `SUCCEEDED / FAILED / CANCELLED` |
| 产品切换 `ProductState` | `UNKNOWN=0, RUNNING=1, SUCCEEDED=2, FAILED=3, INTERRUPTED=4` | `SUCCEEDED / FAILED / INTERRUPTED` |

对应定义：[导航](../proto/messages/navigation.proto)、[地图与产品事务](../proto/messages/maps.proto)。暂停/恢复 RPC 已接现有任务 owner，接纳后仍需观察 `PAUSED` 等任务状态确认执行结果。产品切换成功还需 owner 的 `active=true` 证据。

`UNKNOWN` 与任务查询的 `found=false` 表示当前没有足够状态证据，不等于任务失败。只有 owner 明确报告不存在，才使用 `NOT_FOUND`。成功查询到 `FAILED` 任务时，查询调用码是 `OK`，执行状态是 `FAILED`；两者各自描述自己的事实。

`wait()` 便利方法可以用错误返回非成功终态，但必须保留原始终态、操作身份和原因。取消、失败、中断不得只剩相同的 `OperationFailed` 加拼接文本。本地等待超时只结束等待，不自动取消远端任务。

机器人运行状态按领域分别接入：`system` 管模式与就绪，`motion` 接身体和运动状态，`localization` 接定位状态。本轮不凭借厂商枚举新增 DOSO 尚无实际来源的机器人状态。

## 具体原因与展示文字

`Code` 用于通用分支；`reason_code` 保留 owner 明确给出的稳定原因；`message` 用于展示。任务、操作和请求 ID 继续使用既有身份字段。稳定原因只由其业务责任方定义，Gateway 转交，SDK 不再维护第二套业务判断。

例如现有 owner 已产生：

| `reason_code` | 调用码归类依据 |
| --- | --- |
| `invalid_goal`、`invalid_map_name`、`invalid_operation_id` | 请求参数错误：`INVALID_ARGUMENT` |
| `invalid_client_id` | 握手缺少客户端标识：`INVALID_ARGUMENT` |
| `protocol_mismatch` | 请求协议版本与服务不兼容：`FAILED_PRECONDITION` |
| `operation_not_found` | 已确认不存在：`NOT_FOUND` |
| `control_lease` | 执行方明确拒绝控制权：`PERMISSION_DENIED` |
| `safety_stop`、`active_map_mismatch` | 当前执行条件不满足：`FAILED_PRECONDITION` |
| `map_save_in_progress` | 已接纳的地图保存仍在进行：`OK`，继续读取 `MapState` |
| `map_save_unsupported` | 操作不支持：`UNIMPLEMENTED` |
| `map_service_unavailable`、`native_command_unconfirmed` | 服务/回执不可用：`UNAVAILABLE`；后者仍需保留未知提交事实 |

这些归类依据 owner 完整结果中的接纳和执行事实，不能仅按原因字符串映射；`map_save_in_progress` 就可能是正常进行中的说明。这不是已完成的全量映射。原生 owner 新增或不在已知集合中的原因按原值保留，错误分类信息不足时使用 `UNKNOWN`；不把未知故障默认为成功。

## 实现与边界

- [`Error`](../src/error/mod.rs) 提供 `code()`、`reason_code()`、`details()`；后两者直接读取原始 RPC 详情或 `Terminal`，不复制维护终态数据。
- [`ErrorDetail`](../proto/messages/error.proto) 通过 `doso-error-bin` 传递，包含明确的 `reason_code` 和原始响应，保留请求、任务身份及原生 ACK 等证据。gRPC 标准码仍在标准 status 中；没有自定义详情也能使用原有错误处理。损坏详情会显式追加解码诊断，保留原 RPC 码和未知提交语义。
- [`Receipt`](../proto/messages/common.proto)、[`Task`](../proto/messages/navigation.proto)、[`MapOperation` 和 `ProductOperation`](../proto/messages/maps.proto) 增加 `reason_code` 字段，原字段号和枚举值保持。`reason` 继续承接原有说明，未明确分类的原因不会被猜成机器码。
- `task()` 返回合法状态，包括失败、取消和 `UNKNOWN`；`wait()` 对失败终态附带完整 `Terminal`，遇合法 `UNKNOWN` 继续等待。未知枚举整数、回执身份错配仍报告协议错误。

通用 HTTP → gRPC 状态映射继续尊重已有上游分类，不按原因字符串重新判定业务。例如当前 owner 的 `map_save_unsupported` 返回 HTTP 409，因此调用码仍为 `FAILED_PRECONDITION`，稳定原因现已完整传递；上表的 `UNIMPLEMENTED` 是后续由责任方完善的目标分类。整机鉴权、未接入能力和机器人实测状态不因返回合同完善而被视为已完成。
