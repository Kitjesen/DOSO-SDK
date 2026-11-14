# 功能清单与验收状态

更新：2026-11-13。本页是功能进度的统一入口；方法签名由 [API 手册](api/README.md) 维护，消息字段由 [proto/messages](../proto/messages/) 定义，运行命令与最近测试结果见 [TESTING.md](../TESTING.md)。新增功能时同步这三处，不再另建平行清单。

“本地已测”表示真实 Rust/gRPC 或 Python 服务边界使用测试源通过相应检查，不表示机器人动作通过。下面逐项区分已有代码、数据/执行责任方和现场证据。未列现场通过的功能均未完成真机验收。

## 连接与公共合同

| 功能 | 接口或代码 | 责任方 | 验证与限制 |
| --- | --- | --- | --- |
| 连接、协议握手、三项身份 | `connect`、`with_config`、`system().info()`；[client](api/client.md) | Rust Tonic、System.Handshake；身份读设备配置与发布元数据 | 本地已测版本不兼容、缺身份字段、拒绝和超时；现场观察桥三项均缺失，正式机端握手未验收 |
| 显式重新连接和握手 | `reconnect(&mut self)`；[实现](../src/client/mod.rs) | 原 Endpoint、同一凭据和 TLS 配置 | 本地检查新 TCP、刷新身份、失败保留旧快照；已知 UUID 改变/消失拒绝。既有流由应用重建，不重放命令、不自动申请控制 |
| 凭据与加密 | `Config.api_key`、`tls_ca_certificate` | gRPC metadata、系统根/私有 CA、机器人 listener | 本地已测鉴权与 TLS；设备正式证书/ACL 和正式端点部署未验收 |
| 错误、回执、未知提交 | [Error/Receipt 合同](results.md) | 原业务 owner 与 SDK | 本地已测请求身份、标准码、结构化原因、终态和应答丢失；接纳不等于动作完成 |
| 订阅生命周期 | `watch_*`、Teleop、ExternalControl | 服务共享最多 8 个长流名额 | 本地已测取消、源错误、断流和额度释放；IMU/关节只有 60 秒端到端现场记录，长稳未通过验收 |

## system

[接口](api/system.md) · [Rust](../src/system/) · [消息](../proto/messages/system.proto)

| 功能 | API | 实际来源 | 验证与限制 |
| --- | --- | --- | --- |
| 固件版本、机器人版本、永久 ID | `info` | 运行发布元数据、RobotConfig/RunPlan 型号、`DOSO_DEVICE_ID` | 本地来源与格式已测；研发机已写 UUID，正式机端 SDK 读回未测 |
| 服务响应、运行摘要 | `ping`、`state` | Gateway | 本地已测；ping 不证明可运动 |
| 类型化整机状态 | `robot_status` | Product 状态、就绪、native 软件急停 | 本地已测；硬件急停与完整综合健康无统一来源，保持未知 |
| 健康与故障覆盖 | `health`、`faults`、`watch_faults` | driver.control、driver.health、navigation.control_loop | 本地已测来源覆盖与缺失；空故障列表不证明整机健康，BMS 等未覆盖 |
| 数据与运动就绪 | `readiness`、`readiness_state` | 现有 readiness owner | 本地已测类型转换和未知字段；现场整机就绪未验收 |
| 能力、会话、产品模式 | `capabilities`、`session`、`product_status` | ProductControl、当前 Host 配置 | 本地已测公开字段投影；不暴露云管理凭据，不允许设备修改云归属/授权/规则 |
| 电池与充电状态 | 只有 `BatteryState` 消息 | 尚无已确认真机 BMS 适配器 | **未实现读取 RPC**。没有用电机电压或模拟机 0 值补电量 |

## motion

[接口](api/motion.md) · [Rust](../src/motion/) · [消息](../proto/messages/motion.proto) · [外部控制器消息](../proto/messages/controller.proto)

| 功能 | API | 实际责任方 | 验证与限制 |
| --- | --- | --- | --- |
| 站起、坐下 | `stand_up`、`sit_down` | native 仲裁 → lingtu-driver → Brainstem | 本地准入/ACK 已测；真机动作、与请求关联的动作完成及中途停止未验收 |
| 电机输出门 | `enable_motors`、`disable_motors`、`control_status` | 同上，Brainstem 提供独立读回 | 本地拒绝、身份与映射已测；未执行现场使能/失能 |
| Profile 查询与切换 | `profile`、`switch_profile` | Brainstem profile 事务，经 native/driver 仲裁 | 本地已测；要求 Grounded、电机关闭、释放控制。现场加载与失败恢复未验收 |
| 控制权申请与释放 | `claim_control`、Teleop `claim`/`release_control`/`close` | native operator session、driver lease | 本地已测拒绝、再申请、释放与断线清理；现场争抢和 lease 过期未验收 |
| 当前控制权 | `control_state`、`watch_control_state`、Teleop `control_state` | native OperatorMotionStatus | 本地已测最多 50Hz 快照与过期未知；不是不丢失的逐次失权事件日志 |
| 受控连续速度 | `teleop`、`publish_velocity`、`send_velocity` | 双向 gRPC → native 仲裁 → driver | 本地 50Hz 发布与独立 ACK 已测；机体系 XY m/s、yaw rad/s，现场遥操作与延迟未测 |
| 停止、急停、复位与恢复 | `stop`、`emergency_stop`、`clear_emergency_stop`、`resume` | 原安全链与 native owner | 本地已测；`resume` 是恢复接收新命令，不是摔倒自恢复。Grounded/Transitioning 无 walking lease 时 stop 仍可能 `lease_missing`，物理停稳未验收 |
| 身体、电机与运动状态 | `body_state`、`watch_body_state`、`control_status`、`status`、`watch_state[_at]` | Brainstem 只读源、Gateway 状态投影 | 本地已测；身体为源事件，运动缓存目标 1–100Hz，不代表传感器新样本 |
| 速度与里程计 | `velocity`、`watch_velocity`、`odometry`、`watch_odometry` | SLAM/定位里程计缓存 | 本地映射、坐标与缺失已测；里程计 **10Hz**，速度高频流可重复约 10Hz 源样本 |
| Builtin/External 接管 | `controller`、`external_control`；`publish_joints`、`enable`、`disable`、`close` | native → driver → Brainstem RobotExternalControl | 本地会话、逐关节目标、输入窗口、拒绝、断线关闭输出已测；真机策略接管和硬件输出未测 |
| Euler、站高、步态、速度档位 | 尚无公开整机 API | 需 Brainstem/driver 实际执行能力与范围 | **未实现**。Brainstem Set/GetSpeedMode 当前返回 UNIMPLEMENTED；未生成空壳成功接口 |
| 摔倒恢复、阻尼 | 尚无完整整机 API | driver 当前不支持；Brainstem Recovery 有独立受控会话 | **未接通**。需贯通 native/driver 会话、心跳和中断，再接 SDK；不能绕过最终仲裁直连输出 |

## localization

[接口](api/localization.md) · [Rust](../src/localization/) · [消息](../proto/messages/localization.proto)

| 功能 | API | 实际责任方 | 验证与限制 |
| --- | --- | --- | --- |
| map 位姿与持续观察 | `pose`、`watch_pose` | LingTu SLAM/定位缓存 | 本地坐标、过期与错误已测；目标最高 100Hz 缓存推送不等于 SLAM 100Hz；现场定位未验收 |
| 定位质量与状态 | `status`、`state` | 当前定位 owner | 本地未知状态、fresh 与缺失映射已测；完整源时钟仍待补 |
| GNSS 地理位置 | `location`、`watch_location` | 已装配 GNSS 来源 | 本地 1Hz、无 fix 不带旧坐标已测；真机 GNSS 型号与来源未验收 |
| 全局/种子重定位 | `relocalize` | SLAM/定位 owner | 本地参数、回执与错误已测；实际地图匹配、失定位恢复未验收 |

## maps

[接口](api/maps.md) · [Rust](../src/maps/) · [消息](../proto/messages/maps.proto)

| 功能 | API | 实际责任方 | 验证与限制 |
| --- | --- | --- | --- |
| 地图清单 | `list` | mapd | 本地已测；真机地图服务未验收 |
| 保存、查询、等待结果 | `save`、`operation`、`wait` | mapd 保存事务 | 本地成功/失败/超时/身份已测；实机落盘、重启恢复未验收 |
| 加载地图、进入建图 | `load`、`start_mapping` | ProductControl switch | 本地 session 校验与事务已测；可能重启 Gateway，现场切换后补查原事务未验收 |
| 产品事务补查 | `product_operation`、`wait_product_operation` | ProductControl | 本地终态、拒绝与未知提交已测；重连不自动重发切换 |
| 保存地图 PCD | `point_cloud` | mapd 保存文件 | 本地分块、大小/身份、错误与取消已测；不是实时原始 LiDAR |
| 保存地图栅格 | `grid` | mapd 栅格产物 | 本地尺寸、未知值、来源拒绝已测；不替代三维规划 |
| 地图编辑与独立建图状态流 | 尚无完整公共合同 | 对应地图/SLAM owner | **未完成**；现有保存操作状态与 Product 状态可查，未伪造统一建图进度 |

## navigation

[接口](api/navigation.md) · [Rust](../src/navigation/) · [消息](../proto/messages/navigation.proto)

| 功能 | API | 实际责任方 | 验证与限制 |
| --- | --- | --- | --- |
| XYZ/yaw 到点与任务限值 | `go_to`、`go_to_with_options` | 原 GoalService → nav.commands → native navd | 保留原调用；新增可选 m/s 限速、米制到达半径；本地验证参数传递，实机到点未测 |
| 只读规划预览 | `preview_plan` | ControlCommandService → native preview_plan | 本地验证没有创建/改变任务；返回真实可行性、XYZ 路径、距离/耗时。只读预览不授权运动 |
| 任务身份与终态 | `task`、`watch`、`wait` | native 导航任务投影 | 本地终态/身份/错误已测，任务快照目标 50Hz；等待超时不自动取消 |
| 暂停、恢复、取消 | `pause`、`resume`、`cancel` | 原任务控制 owner 与运动 guard | 本地同任务身份与拒绝已测；实机停稳/恢复未测 |
| 运行中限速 | `set_speed_limit` | 原 control_task(speed_limit) → native navd | 本地参数和回执身份检查；不绕过产品速度上限，现场效果未测 |
| 导航状态与路线 | `status`、`path`、`route`、`watch_path` | 原 nav 状态/路径缓存 | 本地 XYZ/新鲜度与流已测；旧路径高度回退规则见 API，不声称新鲜 3D 测量 |
| 多航点巡检 | 归 `tasks` 的巡检入口 | 已有 nav.inspection 与 native task owner | 不在 SDK 新建顺序 go_to 循环或第二套执行器，见下方 tasks 记录 |

## observations

[接口](api/observations.md) · [Rust](../src/observations/) · [消息](../proto/messages/observations.proto)

| 功能 | API | 实际来源 | 验证与限制 |
| --- | --- | --- | --- |
| IMU 源事件 | `watch_imu` | Brainstem ListenImu → 只读适配器 | 本地已测；2026-11-12 经计算机观察桥 60 秒约 100.015Hz。加速度源缺失，非正式机端部署验收 |
| 单关节/整帧事件 | `watch_joints` | Brainstem ListenJoint | 本地已测；同次现场 16 轴各约 99.931–99.965Hz，最大间隔约 108.582ms。不拼造同步帧；索引见[关节编号](api/observations.md#doso-thunder-16-轴编号) |
| 电机状态、温度、原始错误 | `motors` | Brainstem GetMotorState | 本地字段与错误已测；无统一有效位和采样时间，离线默认值不证明健康 |
| 电机母线电压 | `motor_voltages` | Brainstem CAN 参数查询 | 本地已测；0 也可能超时，不是 BMS，不推算 SOC |
| 相机状态与 JPEG | `camera_status`、`camera_frame`、`watch_camera` | 现有 JPEG relay、媒体访问规则 | 真实 RTSP→临时桥→SDK 单帧已通过；正式 5052 未部署，go2rtc JPEG relay 当前阻塞 |
| 类型化帧与源相机内参 | `camera_sample`、`watch_camera_samples`、`camera_calibration` | relay 发布序号、真实相机 CameraIntrinsics | 真实临时桥连续 10 帧/序号 1–10 已通过；正式 relay、内参对应关系、手机渲染仍未验收 |
| 实时扫描/地图显示点云 | `watch_point_cloud`、`CloudFrame::decode` | CloudViewer 显示发布器 | 本地缓存/reset/颜色/跨线程/超大帧已测；PCLD v2 为量化预览，地图约 4Hz/扫描约 10Hz，真机流未验收 |
| 场景摘要 | `scene` | 当前场景图缓存 | 本地已测；取决于 Host 所装配感知服务 |
| 足接触、足力、光照 | 只有 `FootContact`、`LightSample` 消息 | 未找到实机传感器适配器 | **没有读取 RPC**；设备选型和来源待确认 |
| 采样/上报配置、码率、原始点云 | 当前无统一写 API | 需设备/媒体 owner 提供配置和反馈 | **未实现**。现有 watch 不改变传感器采样率；不把 SDK 丢帧限速当硬件配置 |

## tasks

[接口](api/tasks.md) · [Rust](../src/tasks/) · [消息](../proto/messages/tasks.proto)

| 功能 | 接口范围 | 实际责任方 | 验证与限制 |
| --- | --- | --- | --- |
| 自然语言/高层指令 | `send_instruction` | 现有 AgentPlanner/指令入口 | 本地准入、原文本、回执与未知提交已测；没有 Run ID 时不能声称可追踪 |
| 当前 Agent Run | `get_run`、`get_run_state`、`watch_run`、`Run::state` | 当前 Agent Run owner | 本地类型/状态/50Hz快照/取消订阅已测；不是永久历史数据库 |
| 取消 Agent Run | `cancel_run` | 原任务 owner | 本地请求身份、拒绝和错误已测；实际任务取消未验收 |
| 巡检路线与多航点执行 | `inspection_routes/inspection_route/put_inspection_route/delete_inspection_route`；`start_inspection/inspection_tasks/inspection_task/watch_inspection/pause_inspection/resume_inspection/cancel_inspection/inspection_report` | 原 nav.inspection、native task owner、现有 journal/录制流程 | Rust/gRPC 与共享 owner 接线已完成，本地合同和原 HTTP 回归已测；真实轨迹、证据采集和物理停止未验收。不能把路线保存视为执行完成 |
| 巡检留存任务列表 | `inspection_tasks`，可按地图/路线过滤并包含终态 | 原 InspectionTaskTimeline | 本地过滤、终态、条数钳定、实际retention与时间已测；这是有限留存投影，不是Agent历史数据库 |
| 统一跟随、Agent 历史与多轮续接 | 当前无稳定统一类型接口 | 对应任务 owner | **未完成**；文本能表达某意图不代表具备该功能的稳定 API |

## 部署与交付

| 项目 | 已有内容 | 还未验收 |
| --- | --- | --- |
| Rust 交付 | GitHub `rust` 分支、Cargo Git/path 依赖、Linux/Windows CI、打包验证、API/示例 | 正式 V1、客户独立接入、命名型号兼容表 |
| Android | 独立 Rust bridge、host 合同测试 | APK/手机实装、真实相机/点云渲染与性能 |
| 研发机身份 | THUNDER-DEV-001；永久 UUID `d82a831a-2e86-4b46-975b-3d42473583b7` 已登记并写入配置 | 正式 Gateway 三项身份读回、固件统一发布；不等于已绑定客户或开启云 |
| 正式机端链路 | 源码已有 gRPC → 现有 owner 接线；现场曾通过计算机观察桥读取真实 Brainstem | 完整 LingTu Host/ProductControl/Gateway 安装；上次现场只读检查 13146/5052 未监听 |
| 云端 | SDK 仅本地操控；云端禁止远程运控，设备没有云管理权 | 独立常驻云、许可/断云/本地重开、公网视频等由 robot-cloud 的进度记录验收，不算 SDK 已完成能力 |

现场历史事实及失败原因保留在 [TESTING.md](../TESTING.md#现场记录)，不把新源码、本地测试或短时连续读取写成整机已交付。

## 下一步验收顺序

1. 在允许变更现场运行时后，安装正式机端服务，验证三项身份、权限/证书、只读源及断连重建；先做 15 分钟 IMU/关节并发长稳。
2. 在具备现场动作条件时，分别验收站坐、电机门、50Hz 遥操作、失权/断线停止，再验收 External 接管。已接纳姿态的中断能力必须由底层解决。
3. 用真实地图验收保存/加载/重定位、预览/到点/限速、暂停/恢复/取消和巡检，逐项记录原任务身份与物理停止。
4. BMS、足端、光照、外设/灯光和充电桩先确定设备协议与 owner，再实现适配器和 SDK；姿态/步态、Recovery/阻尼先补真实执行能力，不能在客户端伪造。
