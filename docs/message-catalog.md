# SDK 消息清单

本页面向 App 和 SDK 开发者，说明需要接收什么数据、当前协议覆盖到哪里，以及还缺少哪些真实数据源。`.proto` 是消息定义的唯一来源，本页解释用途和边界，不另写一套 Rust 消息结构。

定义了消息格式、提供了 RPC、机器人端接通了来源、通过实机验收，是四个不同阶段。本页不以格式存在推断设备能力。

## 定义的位置

- `proto/*.proto`：当前七领域的 RPC。原文件通过 `import public` 导出迁移后的消息，保留原服务入口和类型可见性。
- `proto/messages/*.proto`：全部消息、枚举和公共错误定义的唯一位置，按领域及独立职责划分；`controller.proto` 单独维护外部运控消息，由 `motion.proto` 导出。
- `src/<领域>/`：Rust 客户端操作与必要解码；生成消息位于 `doso_sdk::rpc`。公共类型继续从 `.proto` 生成，避免手写重复维护。

已有类型从原文件迁入 `messages/`，不保留重复定义。原 RPC 继续使用相同消息全名和字段编号，目录迁移本身不新增机器人能力。当前服务端和 Rust SDK 使用同一协议包 `doso.v1`，消息组织方式不意味着有七个独立连接或进程。

功能实现和测试状态统一见[功能清单](features.md)，本页维护消息及来源语义。

## 当前 RPC 已使用的消息

| 领域 | 已有主要消息 | 接收方式 | 已明确的内容与缺口 |
| --- | --- | --- | --- |
| `system` | `SystemInfo`、`HandshakeReply`、`Ping`、`Snapshot`、`ProductStatus`、`ReadinessState`、`RobotStatus`、`FaultReport` | 握手、单次查询和故障快照流 | 系统快照仅投影公开业务字段；`product_status()` 返回类型化当前模式与必要切换校验值，不包含运行配置和进程证据。故障包含来源覆盖范围，综合健康与硬件急停仍保留未知 |
| `motion` | `BodyState`、`BodyKind`、`BodyTransition`、`BodyControlStatus`、`ProfileInfo`、`SwitchProfileRequest`、`ControlState`、`Snapshot`、`TeleopRequest`、`TeleopReply`、`TeleopVelocity` | 查询、身体状态事件流、权限与运动快照流、遥操作双向流 | 控制权与速度接现有 native 仲裁，速度回执区分队列接纳和最终输出确认，实际频率待测量 |
| `motion` 外部运控 | `ControllerMode`、`ControllerState`、`JointCommand`、`JointTargets`、`ExternalControlRequest`、`ExternalControlReply` | 控制器查询与双向关节目标流 | 消息在 [controller.proto](../proto/messages/controller.proto) 定义；逐关节目标包含q/dq/kp/kd/tau，队列接纳与机器人帧验证分别报告，调用及关节顺序见[外部运控API](api/motion.md#自定义控制器) |
| `localization` | `PoseSample`、`Pose`、`Snapshot`、`LocalizationState`、`GeographicLocation`、`GeographicFix`、`GeographicState` | 查询、位姿流与 1Hz 地理位置流 | 地图位姿与 WGS84 经纬度分别表示；GNSS 保留源时间、可用状态和未知精度，无 fix 不带旧坐标；详见[地理位置 API](api/localization.md#地理位置) |
| `maps` | `MapRecord`、`MapList`、`MapOperation`、`ProductOperation`、`MapContentInfo`、`MapContent`、`MapGrid` | 查询；点云文件为分块流 | 地图列表、保存/切换状态、PCD 读取及保存的二维栅格查询已接 mapd |
| `navigation` | `Task`、`TaskState`、`Snapshot`、`NavigationPath`、`PlanQuery`、`PlanPreview`、`TaskSpeedLimit` | 查询、任务与路线流 | 任务有明确字段；`route()`、`watch_path()` 使用标准 XYZ 路线，旧 `path()` 和导航状态仍是通用结构 |
| `observations` | `ImuSample`、`JointSample`、`MotorStates`、`MotorVoltagesResponse`、`CameraFrame`、`CameraSample`、`CloudFrame` | IMU、关节、相机和点云为流；电机状态及电压为查询 | 相机 Sample 流包含服务内发布序号；各数据的时间、有效性、索引和编码边界见下表 |
| `tasks` | `Instruction`、`Run`、`TaskRunState`、`InspectionBinding`、`InspectionRoute`、`InspectionTask` | 命令回执、运行查询和当前 run 与巡检任务50Hz快照流 | `get_run_state()` 和 `Run::state()` 共用当前 AgentPlanner 状态解析，`watch_run()` 每 20ms 交付当前 `Run` 快照，允许内容重复 |

`Snapshot.fields` 和各消息的 `details` 使用 `google.protobuf.Struct`。核心业务字段逐项固定前，调用方不能仅凭类型知道全部字段、单位和可选性。

巡检消息单独位于 [messages/inspection.proto](../proto/messages/inspection.proto)，由 tasks.proto 公开导出。路线沿用既有 schema，任务消息区分接纳、执行事实和终态，详见[巡检 API](api/tasks.md#巡检路线与任务)。

## 本轮新增的标准消息

下表逐项说明标准格式和接线状态；格式存在本身不能证明来源可用。

| 消息 | 定义文件 | 用途 | 接线状态与来源边界 |
| --- | --- | --- | --- |
| `SampleTime` | [common.proto](../proto/messages/common.proto) | 明确数据时间和时钟语义 | 公共时间格式；现有 `source_elapsed`、点云时间的含义仍按原合同解释，不能因新增类型冒充硬件采样时间 |
| `RobotStatus`、`HealthStatus` | [system.proto](../proto/messages/system.proto) | 模式、就绪、健康、软硬件急停与原因 | `system().robot_status()` 已接模式、就绪和新鲜native软件急停；综合健康及硬件急停仍无统一来源，保持未知 |
| `BodyControlStatus`、`BodyControlOwner` | [motion.proto](../proto/messages/motion.proto) | 身体、电机门、关键故障、源owner与lease状态 | `motion().control_status()` 直接读取Brainstem，只读且不申请控制；源无采样时间，不宣称硬件采样新鲜 |
| `ProfileInfo`、`SwitchProfileRequest` | [motion.proto](../proto/messages/motion.proto) | 当前profile、可用名称及说明，带命令身份的切换请求 | profile只读透传Brainstem。switch_profile经native与driver执行，要求Grounded、电机关闭和释放控制，ACK后通过独立profile查询确认 |
| `TeleopVelocity`、`TeleopRequest`、`TeleopReply` | [motion.proto](../proto/messages/motion.proto) | 机体 XY 速度 m/s、Z 轴角速度 rad/s、deadman、输入窗口与回执 | `motion().teleop()` 双向流已接 native operator session。`publish_velocity()` 不等逐条 ACK，输入携带固定过期窗口与递增序号；`send_velocity()` 可等待单条回执，`ingress_accepted` 表示 Gateway 队列接纳，`final_output_confirmed` 提供 hold/release 确认 |
| `ReadinessState` | [system.proto](../proto/messages/system.proto) | 整体、数据、运动就绪及原因 | SDK 从现有 readiness 查询投影，不额外增加 RPC；缺失布尔值保持缺失 |
| `ControlState` | [system.proto](../proto/messages/system.proto) | 权限空闲、owner/epoch、当前会话是否持权、hold、接收年龄与新鲜度 | 接原生 DDS `OperatorMotionStatus`。全局查询与流只读，Teleop反馈额外匹配会话身份。过期时available/held_by_client/holding缺失；available仅表示原生权限空闲，申请结果以ACK为准 |
| `BatteryState` | [system.proto](../proto/messages/system.proto) | 电量百分比、充电状态、电压 V、电流 A、温度 °C；电流正值表示放电 | 真实 BMS 来源尚未确认；只定义格式，不从电机母线电压推算电量，不填虚构数据 |
| `Fault`、`FaultSeverity`、`FaultReport` | [system.proto](../proto/messages/system.proto) | 当前故障、等级与已观察/不可用来源 | `faults/watch_faults`接driver控制、driver健康、navigation控制循环三个来源。无已知故障时health为UNKNOWN，电机关闭属于正常输出门状态；BMS与原始电机完整故障上报仍待接入 |
| `VelocitySample` | [motion.proto](../proto/messages/motion.proto) | 线速度与角速度 | `velocity/watch_velocity` 已接定位里程计输入的body三轴线速度和角速度，含source与接收年龄；源时间未知时保持缺失 |
| `OdometrySample` | [motion.proto](../proto/messages/motion.proto) | 连续运动位姿与速度 | `odometry/watch_odometry` 已接定位里程计缓存，订阅按10Hz交付（real原生来源为SLAM），含完整姿态与body三轴速度、source及接收年龄；源时钟域未知时time缺失 |
| `LocalizationState` | [localization.proto](../proto/messages/localization.proto) | 定位状态、当前地图、就绪、新鲜度与原因 | `localization().state()` 已映射现有定位 owner，未知状态保留 `source_state`，未知时间或布尔值保持缺失 |
| `MapGrid` | [maps.proto](../proto/messages/maps.proto) | 栅格地图：地图 ID、内容代次、宽高、米制分辨率、原点、坐标系与 cells | `maps().grid()` 已接 mapd 保存产物；cells 自原点按行存储，x 先变化再变化 y，0–100 为占据值，255 表示未知；源无采样时间 |
| `NavigationPath` | [navigation.proto](../proto/messages/navigation.proto) | 导航路线 | `route()` 和 `watch_path()` 读取地图坐标系缓存；旧兼容路径上游可能将缺少的高度填零，不能一概视作实测高度 |
| `CameraCalibration` | [observations.proto](../proto/messages/observations.proto) | 源彩色图尺寸、焦距、主点、坐标系及畸变 | `camera_calibration()` 已接已发布的相机内参；缺来源返回 UNAVAILABLE，不能直接视为旋转后 JPEG 的标定 |
| `CameraInfo` | [observations.proto](../proto/messages/observations.proto) | 相机设备与成像信息 | 编码已知为JPEG；ID、尺寸和坐标系为optional，来源未知时为None |
| `CameraSample` | [observations.proto](../proto/messages/observations.proto) | 复用 `CameraFrame` 和 `CameraInfo`，附带时间和可选帧序号 | 查询和流已接 JPEG 来源；流序号为服务发布计数，时间和尺寸等未知信息未接入，需设备验收 |
| `LightSample` | [observations.proto](../proto/messages/observations.proto) | 光照强度 lux 和时间 | 仅定义格式，传感器支持和真实来源待确认 |
| `TaskRunState`、`TaskRunStatus` | [tasks.proto](../proto/messages/tasks.proto) | 高层任务运行状态 | SDK 从当前 AgentPlanner 查询或 `Run` 流解析状态、原因与摘要。当前 run 的 50Hz 快照流已接通，历史查询、统一任务类型、阶段与进度仍待来源支持 |

地图已有 `MapRecord`、`MapOperation`、`ProductOperation` 和内容块等类型；`MapGrid` 已接 mapd 的保存栅格产物。统一建图状态仍需继续接线。

## 当前观测格式的语义

| 数据 | 格式与单位 | 时间、坐标和有效性 |
| --- | --- | --- |
| IMU | `ImuSample`：角速度 rad/s、加速度 m/s²、姿态四元数 | 字段声明姿态为 world→body；四元数字段名为 w/x/y/z。`source_elapsed` 是 Brainstem 服务映射事件时的相对时间，不是硬件采样时间；未提供的消息字段保持缺失，不拼造值 |
| 关节 | `JointSample.joints`：ID、位置 rad、速度 rad/s、力矩 N·m、状态码 | 一条消息可能是单关节事件，也可能是上游整帧；不能默认所有关节同时采样。`source_elapsed` 与 IMU 同属源服务相对时间；Thunder 当前源码的统一索引与原始报告 ID 对照见[关节编号](api/observations.md#doso-thunder-16-轴编号)；旧部署转换和实际方向标定仍待现场确认 |
| 电机诊断 | `MotorStates`：每电机 ID、online、状态码、温度 °C、原始故障码 | 当前为查询，没有统一报告有效位和采样时间；离线默认值不能证明设备健康 |
| 电机电压 | `MotorVoltagesResponse.volts`：V | 查询触发底层 CAN 参数读取；0 V 也可能是未响应。它不是电池电量或 BMS 状态；数组索引需要与型号电机映射一致 |
| 身体状态 | `BodyState`：kind、transition、gesture_name | 先返回当前状态，再接收状态事件；无固定推送频率、无源采样时间；它不表示运动命令已完成或已物理停稳 |
| 地图位姿 | `PoseSample`：x/y/z 为 m，yaw 为 rad，`frame_id` 为坐标系 | 有可缺省接收年龄与fresh；已知过期/定位丢失会终止流。它不是硬件采样时间，重复推送不代表新采样 |
| 相机图像 | `CameraFrame.jpeg`：JPEG 文件字节 | `CameraSample` 流含 JPEG 编码和服务发布序号；采样时间、相机 ID 等仍按源缺失表达；CameraCalibration 已接源彩色图内参，JPEG 预览变换不保证与其一致 |
| 实时点云 | `CloudFrame.pcld`：PCLD v2 二进制帧；Rust `decode()` 生成 `PointCloud` | 解码后有米制 XYZ、可选 RGB、frame_id、epoch、sequence、stamp_s、stream_kind、scale_m；属于显示预览，经过量化和显示采样，不承诺完整原始雷达数据 |
| 保存地图点云 | `MapContent`：第一条为 ID、可选内容代次/坐标系和字节数，后续为 PCD 块 | 文件流正常结束且总字节数匹配才表示读取完成；读取不等同于激活地图或恢复建图 |

实时点云首帧可能来自缓存，慢消费者可能跳过旧帧。源时间缺失时，`stamp_s` 可能回退到机器人服务时间。`Reset`、坐标系或场景代次变化应清理旧显示，不能跨坐标系累计点云。

## 标准格式应覆盖的显示内容

以下是需要作为标准消息覆盖的数据范围。新增 `messages/` 定义的内容仍需逐项接到查询、流或事件接口；字段不能提供时必须表达缺失，不能返回看似正常的零值。

| 数据类别 | App 可以显示什么 | 接收方式 | 机器人端来源或当前缺口 |
| --- | --- | --- | --- |
| 综合状态 | 当前模式、健康、就绪与不可用原因 | 查询与变化上报 | ProductControl、现有机器人状态投影；需以领域标准字段替代核心动态字典 |
| 控制权 | 权限空闲、当前持有方、本会话持权 | 查询与最高50Hz快照 | 已接申请、重新申请、释放与native权限快照，快照可合并，完整逐次事件仍待扩展 |
| 运动学 | 位置、姿态、机体/世界坐标系线速度与角速度 | 持续流 | 已接定位输入的map→body位姿和body三轴线/角速度，source及接收年龄随样本返回；real源默认状态导出约10Hz，缓存流可重复 |
| 故障 | 代码、来源、等级、描述和来源覆盖 | 查询与最高50Hz快照 | 已接driver控制/健康与navigation控制循环，更多来源和历史生命周期待接入 |
| 电源电压 | 各电机母线电压，单位 V | `motor_voltages()` 查询 | V1 沿用 Brainstem `GetVoltage`；电池百分比和充电状态后续按实际 BMS 来源开放 |
| 定位 | 当前地图、位姿、定位是否可用、状态与原因 | 查询与位姿流 | LingTu SLAM/定位，位姿流已接缓存，类型化状态已映射，源时间仍待完善 |
| 地图 | 当前地图、建图状态、保存进度、栅格内容 | 查询、操作状态、内容流 | mapd、SLAM、ProductControl；保存栅格读取已接 mapd，未知区域不等于自由空间 |
| 导航 | 路线、目标、规划/执行/暂停/恢复/结束状态、原因 | 查询与流/事件 | LingTu navd；路线流、任务暂停/恢复已接现有owner，实际执行仍待实机验收 |
| 相机 | 画面、设备与帧状态 | 图像流和状态事件 | 现有 JPEG relay；编码和服务序号已接，帧标识/采样时间等仍按源表达缺失；CameraCalibration 单独提供已发布源彩色图标定，预览变换另验 |
| 点云 | 扫描、地图预览、坐标对齐与场景重置 | 帧流 | 现有显示源已提供 PCLD v2；标准格式需写清预览性质和量化，不重复维护另一套点云事实 |
| 高层任务 | 当前 run 状态、结果与失败原因 | 查询与 50Hz 快照流 | 已接 AgentPlanner 当前 run，Agent 统一类型/阶段/进度与跟随仍待补；巡检路线、任务流和报告已单独接已有 owner |
| 光照、外设 | 型号真实支持的数据 | 按设备来源确定 | `LightSample` 已定义格式，真实来源/设备支持待确认且无接收 RPC；外设类型按具体设备确定 |

## 频率、时间和未知值

- 运动快照当前默认目标最高 100 Hz，可选 1–100 Hz；它评估的是缓存状态，不改变源端采样。Windows 本机空设备夹具尚未测到实际 100 Hz，机器人端性能待验收。
- 地图位姿和导航路线流目标最高 100 Hz，重复读取缓存；Windows 本机单订阅夹具分别约 66 和 70 帧/秒，不能证明硬件采样频率或现场性能。
- IMU 与关节按源事件交付，100 Hz 是待验证目标，不是 SDK 已控制硬件采样的事实。单关节每秒 100 条与整机同步帧每秒 100 帧不同。
- 身体状态跟随源事件，故障、控制权、导航与高层任务采用最高50Hz状态快照。电池、温度和静态身份沿各自来源更新率交付。
- 相机和点云按各自设备及带宽确定帧率。现有地图显示约最高 4 Hz、扫描显示约最高 10 Hz，不能因运动流调整而宣称已变为 100 Hz。
- `Tasks.Watch` 和 `Navigation.Watch` 以 50Hz 目标持续发送快照。遥操作一次握手后连续发布，ACK 独立接收；机器人每 50ms 推送新窗口，每个窗口有效期固定为 350ms。发布返回本地入队事实，机器人回执独立表示接纳。
- 时间必须说明采样、接收或状态生成的含义和时钟域。未提供硬件采样时间时保持缺失，不把接收时间冒充采样时间。
- 坐标类数据明确 frame_id、方向和单位；没有数据、设备未配置、源暂时失效与数值为零应分别表达。

## 控制消息与结果

当前公共 `Command` 提供 request_id/client_id，`Receipt` 提供接纳事实、关联任务/操作/运行 ID、原因与阶段。命令成功发送、机器人接纳请求、任务最终完成和物理停稳分别判断。

普通停止、运动急停、急停复位、导航目标与任务暂停/恢复/取消、地图保存/切换、重定位和文本指令已有 RPC。受控速度发送通过 `Motion.Teleop` 双向流，hold/close 回执含 native 最终输出确认。

`Motion.StandUp/SitDown` 复用 `Command → Receipt`，SDK 请求超时为 `max(Config.timeout, 15秒)`。Gateway 复用 journal 与 motion guard，沿 `NavCommands.stand/sit → nav DDS → driver → Brainstem` 提交。回执描述接纳，后续身体状态由查询与事件提供。SDK、Gateway 与原生姿态链构建和本地验证已通过，实机动作待验收。

`Motion.Resume`沿同一journal与native恢复通道解除当前模式hold。显式申请使用Teleop新增`claim`动作，`close`释放同一个native会话。`TeleopReply.control`、`ControlState/WatchControlState`按目标50Hz提供可合并快照，逐次失权事件必达仍待补齐。

`Motion.EnableMotors/DisableMotors`复用 `Command → Receipt`，状态读回使用 `BodyControlStatus.motor_output_enabled` 软件输出门，逐电机实际状态由电机诊断提供。`Motion.Profile`返回 `ProfileInfo`，`SwitchProfile`使用 `SwitchProfileRequest → Receipt`，公共操作不附带站坐或重新使能。Profile ACK与独立状态读回分别核查，设备动作仍需现场验收。

所有 gRPC 消息可沿用 API key metadata 和 TLS 通道，服务端在业务处理前验证凭据。证书和私钥由服务端配置，客户端支持系统信任根及额外 PEM CA，接入方式见[使用说明](usage.md#api-key-与-tls)。

## 智身对照范围

参考固定提交 `e8f959a22a6e5714bc299a551b45e1c182ac05c7` 的 [数据类型](https://github.com/zsibot/genisom_robot_sdk/blob/e8f959a22a6e5714bc299a551b45e1c182ac05c7/docs/zh/sdk_type_zh.md) 和 [回调](https://github.com/zsibot/genisom_robot_sdk/blob/e8f959a22a6e5714bc299a551b45e1c182ac05c7/docs/zh/sdk_callback_zh.md)。智身将命令参数、应答、持续上报及事件分开；DOSO 同样保持这些语义，通过 Rust 异步流和 gRPC 表达。

重点对照综合状态、运动/速度、IMU、关节、故障、控制权和任务。DOSO 另外维护 SLAM、地图、定位和导航的数据合同。厂商字段、故障码、坐标系和支持机型不能直接当作 DOSO 设备事实；公开文档中的频率也不作为 DOSO 实机性能承诺。

本清单依据协议和实现审查，未执行实机读取或更改设备部署。
