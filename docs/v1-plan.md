# DOSO SDK V1 产品与实施计划

日期：2026-11-02。面向基于 DOSO 整机开发巡检、科研与二次开发应用的工程师。

当前状态更新于 2026-11-18。逐领域已有功能、测试与未完成项以[功能清单](features.md)为准，复现命令和最近结果见[TESTING.md](../TESTING.md)。设备没有云管理权。SDK 的运动和实时观测直接面向机器狗 Brainstem `13145`；地图、SLAM、导航和高层任务的 `DosoClient`/`5052` 仅作为可选高层服务。云端只读同步状态，不远程运控。

## 产品目标与交付形态

开发者通过独立 SDK 连接机器人，检查能力与就绪状态，选择地图与定位，提交导航目标，追踪指定任务并请求停止。命令接纳与执行完成分别表达，连接、定位和控制权失败有明确原因。

V1 以异步 **Rust 整机客户端**为主。Cargo 包 `doso-sdk`、导入名 `doso_sdk`，使用 **Tonic + Prost + Tokio**。应用通过 Cargo 依赖集成，构建依赖 `protoc`，具体步骤见 [Rust SDK](usage.md)。

七域 gRPC 客户端与 LingTu handler 已完成本地实现，取代原先 Rust HTTP/JSON 预览。机器人端复用现有 owner 与共享领域操作。实际测试及验证边界由 [gRPC 推进记录](grpc-v1-progress.md) 记录；本页不将本地实现等同于仿真或实机验收。

现有 C++ Brainstem 客户端维持独立的底层开发用途；历史 Python 预览与 Rust `0.1.0-alpha.1` 记录作为参考，不再作为当前主线合同。

## 七域范围

| 域 | 当前 gRPC 范围 | 后续 V1 工作 |
| --- | --- | --- |
| system | 只读协议握手与固件版本/机器人版本/机器人 ID、ping、状态、健康、故障查询/快照流、就绪、能力、会话与 ProductControl 状态查询 | 逐台 ID 登记、统一固件交付与健康/就绪的真机验收 |
| motion | 站起/坐下请求、电机使能/失能、profile查询/切换、停止与急停/复位、显式恢复、控制权申请/重新申请/释放与只读快照、身体状态查询/事件、速度与10Hz里程计读取、运动状态流、受控遥操作与外部逐关节控制双向流 | Product 运行模式接线、姿态与步态、控制交接及实机动作验收 |
| localization | 类型化定位状态、map 位姿查询/流、WGS84 GNSS 查询/1Hz流、带种子/全局重定位 | 源时间与坐标语义、真实 GNSS 与实机定位链验收 |
| maps | 列表、保存与操作等待、ProductControl 加载/建图、PCD 下载与保存栅格读取 | 地图编辑、故障恢复与目标平台验收 |
| navigation | map-frame XYZ/yaw 目标、任务查询/流式等待、暂停/恢复/取消、路线查询/流 | 实际导航闭环及暂停/恢复验收 |
| observations | 场景、相机状态与 JPEG 流、预览点云流、IMU/关节源事件、电机状态与电压查询 | 电池电量/充电状态、光照等来源确认，源时间及频率实测 |
| tasks | 指令/当前Agent Run、巡检路线CRUD/执行/50Hz任务流/控制/报告 | Agent历史、统一跟随与多轮续接；巡检真机验收 |

## 实施阶段与验收

| 阶段 | 交付与完成条件 | 当前状态 |
| --- | --- | --- |
| M1 本地开发预览 | Rust 七域 HTTP 预览、Cargo 包、只读示例及本地客户端测试 | 历史基线，见 [Rust 推进记录](history/rust-v1-progress.md) |
| M1b 正式通信合同 | 一套领域协议、异步 Rust 客户端、真实 gRPC handler；关键身份与终态使用明确字段，导航提供任务流 | 已完成本地实现；具体验证见 [gRPC 推进记录](grpc-v1-progress.md) |
| M2 服务接线 | 身体动作、遥操作与控制权交接、暂停/恢复，经实际 Gateway 和 owner 验证 | 站起/坐下、遥操作及暂停/恢复已接 SDK/Gateway，原生姿态构建与本地合同验证通过，显式恢复、控制权管理与权限快照已补本地接口，现场验收待推进 |
| M3 仿真闭环 | 在命名环境验证地图定位、导航、停止、接管与故障，记录执行与状态来源 | 待推进 |
| M4 实机与开发者试用 | 目标型号兼容表、部署配置和安装/排错指南，授权现场验收及独立开发者试用 | 待推进 |
| V1 发布 | 完成所需验收，提供版本化包、变更记录与已知限制 | 未发布 |

本地测试、真实服务接线、仿真运动和实机行为分别记录，不以日期或目录齐全代替验收。

## 当前实现合同

- `DosoClient::connect(...).await` 建立通道并完成观察式协议握手。七域共用 channel、配置与请求元数据，`system().info()` 读取固件版本、机器人版本和机器人 ID 的最近成功握手快照，动态状态单独查询。
- 普通 RPC 默认超时 10 秒，重定位 60 秒，站起/坐下、电机写操作及profile切换使用 `max(Config.timeout, 15秒)`。写请求保留 request_id 供核查可能已执行的操作。
- 导航按 task_id 追踪，`watch`/`wait` 消费服务端任务流；地图保存按 operation_id 等待，产品切换按 request_id 查询。停止本地等待不撤销远端动作。
- 位姿使用明确的 map 坐标系和物理单位；命令与关键结果由 Protobuf 字段定义，诊断快照通过 `Snapshot.fields` 保留现有状态。
- 软件急停回执与物理停稳分别表达。系统健康不覆盖本次事务的失败原因。
- 电机使能/失能与profile切换经native仲裁和driver执行，状态复用control_status与独立profile查询。公共profile切换要求Grounded、电机关闭及控制权释放，调用者显式管理站坐和电机恢复。使能本身只打开输出门，失能本身只撤权并关闭门。
- 协议源为 [`proto/`](../proto/)，Rust 与 LingTu Python bindings 从同一套协议生成，业务规则复用现有 owner。
- 遥操作一次握手后持续发布新输入，后台独立接收回执。输入窗口固定350ms有效。`claim_control` 打开同一会话并申请，`claim` 支持重新申请，`release_control` 复用 close。native lease 为1秒，SDK没有后台保活，首条速度延迟申请继续保留。
- 全局和会话控制权以最高50Hz快照提供，只有会话快照设置 `held_by_client`。快照可合并，过期时 `available` 与 `held_by_client` 保持未知。Agent run与导航任务也按目标50Hz推送，现场频率和时延仍需实测。
- `system.faults/watch_faults` 投影 driver.control、driver.health、navigation.control_loop 三类来源，并显式列出 observed/unavailable 覆盖。没有已知故障时 health 保持 Unknown，电机关闭本身属于正常状态。
- gRPC API key 与 TLS 分别配置，客户端支持系统信任根和私有 CA。部署凭据和服务证书的接入方式见[使用说明](usage.md#api-key-与-tls)。

## 下一阶段推进

接口签名、参数、返回和示例由API手册维护。快速入门提供构建与常用流程，厂商逐项对照由架构文档维护。[API手册](api/README.md) · [厂商对照](architecture.md#连接与操控对照)

对外统一管理固件版本、机器人版本和机器人 ID。三项信息的来源、现有发布入口与交付缺口统一见[机器人信息与固件发布](release.md)。

M1b 的本地合同和 Gateway 启停接线已验证。M2 已通过既有 journal、motion guard 与 native owner 接入姿态请求、受控遥操作及导航暂停/恢复，原生姿态链已完成构建与本地验证，本轮补齐显式恢复、会话控制权管理和只读权限快照，继续推进真实运行环境验收。命令回执描述接纳，后续执行状态由原 owner 提供。

地图加载继续使用 ProductControl `switch` 事务，调用方提供 `expected_product_session_id`，按相同 request_id 查询结果。产品切换可能重启 Gateway，需在真实服务环境验证连接与操作查询恢复。

API key 鉴权与 TLS 的本地验证已完成，未配置凭据和证书时采用开发用明文 listener。下一步验证实际部署配置、失联恢复和目标设备表现，电池电量/充电状态、光照等观测来源及统一巡检/跟随接口按实际 owner 的能力逐项接入。

验证重点是任务身份、拒绝与超时、失联与抢占、地图事务和停止语义。沿用现有有意义的测试；不通过新增平行任务引擎或重复状态来补合同缺口。

## 对标后的能力缺口

完整缺口和每项的来源、接口、测试状态统一维护在[功能与验收清单](features.md)。推进顺序为 Brainstem 直连只读长稳 → 基础操控与停止 → External 逐关节接管与安全回退 → 可选 LingTu 高层服务的地图/导航/巡检闭环 → 按真实硬件能力补姿态、Recovery、BMS 和外设。云端仅验收只读状态链与断云行为。

Rust 主线已提交 GitHub `rust` 分支，支持 Cargo Git/path 依赖并通过跨平台 CI；正式 V1 和客户试用仍未完成。底层不支持的能力不以空壳 API 计为完成。

里程计按产品确定的10Hz交付。当前速度反馈新样本也来自默认约10Hz的定位导出，速度缓存高频推送的来源与频率分别记录。Brainstem独立机身线速度和局部里程计尚无完整输出，后续若有独立需求，再按真实来源接入。

宇树对照范围为固定提交的Go2 SportClient。该版本公开头文件包含Euler、具体步态、RecoveryStand和Damp，BodyHeight与SwitchGait只出现在示例注释中。智身Gait为无步态类型参数的方法。对照采用这些已核实边界。[官方来源与具体方法](architecture.md#连接与操控对照)
