# DOSO SDK 七域真机验收矩阵

更新日期：2026-11-14。本文用于 `rust` 分支的真实机器人验收。它描述验收动作和证据，不把本地 gRPC 合同测试、Gateway 仿真或历史现场记录写成真机通过。

## 验收规则

每个门只使用四种结果：

- `PASS` 表示动作、源数据和安全回退都满足通过标准。
- `FAIL` 表示出现明确错误、越权、错误数据或安全回退失败。
- `BLOCKED` 表示前置条件、机端服务、硬件或网络不可用，尚未执行主体动作。
- `NOT RUN` 表示尚未开始该门。

任何门出现 `FAIL` 或 `BLOCKED`，停止后续依赖该门的动作。每个门单独保存原始日志、请求身份、服务版本、开始时间、结束时间和结果摘要。

### 固定现场信息

| 项目 | 验收值 |
| --- | --- |
| SDK 仓库 | `D:\inovxio\brain\DOSO-SDK` |
| SDK 分支 | `rust` |
| 目标设备 | 研发机 `THUNDER-DEV-001` |
| 设备 UUID | `d82a831a-2e86-4b46-975b-3d42473583b7` |
| Gateway | 现场正式 `5052` 地址，使用真实 API key 和 TLS CA |
| 证据目录 | `D:\DOSO\artifacts\robot-cloud\sdk-real-acceptance-YYYYMMDD` |
| 云端权限 | 只读状态与授权主题，禁止运动、遥操作、电机和通用 gRPC 透传 |

### 通用前置检查

1. 确认机器人放置在空旷区域，旁边有现场急停和人工观察员。
2. 记录机器人型号、机器人版本、固件版本、UUID、Brainstem 版本、Gateway 版本、SDK commit 和证书指纹。
3. 确认 CAN、IMU、相机、电机反馈和地图服务处于已知状态。
4. 确认云端运动权限关闭。所有运动动作只从本地 SDK 发起。
5. 保存 `systemctl`、监听端口、网络地址、配置摘要和 `field-preflight.sh` 原始输出。
6. 每项动作使用新的 request ID。未知回执保留原 request ID，重新读取状态后再判断结果，不重复发送同一动作。

## 1. 连接和 System 信息

| 项目 | 内容 |
| --- | --- |
| 目标 | 证明 SDK 能通过正式 Gateway 完成握手、认证和系统信息读取 |
| SDK 入口 | `DosoClient::connect` 或 `DosoClient::with_config`、`system().info()`、`health()`、`readiness()`、`faults()`、`session()` |
| 执行 | 使用正式 `5052` 地址连接，完成握手后读取身份、健康、就绪、故障和会话。重复连接一次，确认新连接能刷新会话信息。 |
| 通过标准 | TLS 主机名和 CA 校验通过，API key 只发送一次且认证成功，UUID 与现场登记一致，系统信息字段来源明确，未知字段保持未知，健康/就绪查询不取得运动控制权。 |
| 失败处理 | 认证、TLS、协议版本或身份不一致时停止，不进入运动门。 |
| 证据 | `01-system-connect.json`、`01-system-connect.log`、版本与证书摘要。 |

建议命令：

```text
cargo run --example read_status -- https://<gateway>:5052
```

## 2. Motion 的站起、坐下、停止和电机状态

| 项目 | 内容 |
| --- | --- |
| 目标 | 证明本地 SDK 命令经过原有仲裁、回执和安全状态链 |
| SDK 入口 | `motion().stand_up()`、`sit_down()`、`stop()`、`body_state()`、`control_status()`、`observations().motors()` |
| 前置 | System 门通过，身体处于可控的落地状态，电机输出和控制 owner 已记录，现场急停可用。 |
| 执行 | 发送站起，观察身体状态到 Standing。发送坐下，观察回到 Grounded。每个过渡中单独发送停止，记录停止回执、最终身体状态、控制 owner、电机输出和故障。 |
| 通过标准 | 每个动作只有一个 request ID，回执和状态身份一致。站坐状态按真实源变化结束。停止后最终输出关闭或进入合同定义的安全状态，控制 owner 和关键故障字段没有被 SDK 伪造。 |
| 失败处理 | 回执未知、状态不变化、停止未确认、控制 owner 异常或电机故障时立即停止后续运动门，保留现场状态。 |
| 证据 | `02-motion-posture-stop.json`、`02-motion-posture-stop.log`、身体状态流和电机状态快照。 |

## 3. IMU、关节 100Hz 和 Odometry 10Hz

| 项目 | 内容 |
| --- | --- |
| 目标 | 证明观测流的实际频率、连续性、索引和源时间 |
| SDK 入口 | `observations().watch_imu()`、`watch_joints()`、`motion().watch_odometry()` |
| IMU/关节窗口 | 连续 900 秒，同时消费 IMU 和全部关节轴。记录接收时间、源时间、序号、每轴样本数、间隔和错误。 |
| Odometry 窗口 | 连续 600 秒，目标 10Hz。记录 parent/child frame、位姿、速度、源时间和接收间隔。 |
| 建议通过阈值 | IMU 平均频率 `98–102Hz`。每个关节轴平均频率 `98–102Hz`。单次空窗不超过 2 秒。单轴最大接收间隔建议不超过 200ms。Odometry 平均频率 `9.5–10.5Hz`，空窗不超过 3 秒。 |
| 通过标准 | 所有轴都持续有样本，频率和间隔满足阈值，源错误按真实错误交付，取消流后连接资源释放。SDK 不把缓存重复当成新采样。 |
| 失败处理 | 任一流停流、轴缺失、源时间倒退或错误被吞掉时停止长稳结论，记录失败窗口和最后一条有效样本。 |
| 证据 | `03-observations-900s.json`、`03-odometry-600s.json`、原始日志和频率统计。 |

建议命令：

```text
cargo run --example measure_observations -- https://<gateway>:5052 900 both
```

## 4. Localization 和地图状态

| 项目 | 内容 |
| --- | --- |
| 目标 | 证明定位、坐标系、地图目录和地图内容读取来自真实 LingTu 服务 |
| SDK 入口 | `localization().status()`、`state()`、`pose()`、`watch_pose()`、`location()`、`maps().list()`、`grid()`、`point_cloud()` |
| 执行 | 读取定位状态和坐标系。确认当前地图。读取一份栅格和一份 PCD。若现场批准写入，再单独执行地图保存或加载事务。 |
| 通过标准 | 定位质量、地图名、frame、位姿和源时间与现场状态一致。地图列表、栅格尺寸、分辨率、原点、PCD 分块和结束状态可解码。读取失败保留真实错误，不能把缺图当空图。 |
| 失败处理 | 定位失效、frame 不一致、地图事务身份不一致或内容校验失败时停止导航门。保存/加载动作不在默认验收中执行。 |
| 证据 | `04-localization-map.json`、栅格摘要、PCD manifest、地图服务日志和定位状态流。 |

## 5. 点云、相机和视频

| 项目 | 内容 |
| --- | --- |
| 目标 | 证明 SDK 能接收观测数据，媒体授权、元数据和取消行为正确 |
| SDK 入口 | `observations().camera_status()`、`camera_calibration()`、`camera_frame()`、`watch_camera_samples()`、`watch_point_cloud()`、`maps().point_cloud()` |
| 相机执行 | 读取状态和内参。读取单帧 JPEG。持续接收 60 秒类型化帧，记录序号、尺寸、frame、时间和解码错误。 |
| 点云执行 | 分别读取 scan 和 map 视图。持续接收 60 秒点云，记录 PCLD 版本、坐标系、点数、颜色字段、序号、重置和取消。 |
| 视频执行 | 使用正式观看授权，通过 App 或批准的查看器确认画面持续解码。MQTT 只验证源启停消息，媒体帧走视频通道。 |
| 通过标准 | JPEG 可解码，尺寸和内参字段一致。点云帧可解码，坐标系和代次保留，空帧与 reset 有明确语义。取消后流结束且资源释放。视频有持续帧增长、停止观看后源按合同退出。 |
| 失败处理 | 权限错误、帧停流、元数据缺失、点云解码失败或取消不释放时停止该门，不把地图 PCD 当实时点云。 |
| 证据 | `05-camera-pointcloud-video.json`、JPEG/点云摘要、视频观看记录、授权和取消日志。 |

## 6. Navigation 规划预览和一次真实到点

| 项目 | 内容 |
| --- | --- |
| 目标 | 证明导航规划、任务身份、路线、执行状态和到点结果来自真实 nav owner |
| SDK 入口 | `navigation().preview()`、`goal()`、`route()`、`watch_path()`、`watch()`、`wait()`、`pause()`、`resume()`、`cancel()` |
| 前置 | Localization 和地图门通过。现场选择无障碍短路线，确认速度上限、到达半径、急停和人工接管。 |
| 规划执行 | 先执行只读规划预览并保存路线。再提交一个短距离目标，记录 request ID、task ID、路线、状态流和最终状态。 |
| 通过标准 | 预览不创建任务。真实目标只创建一个任务并返回同一 task ID。路线坐标系和点集有效。任务状态按 `accepted → running → succeeded` 或真实失败原因变化。机器人在到达半径内停稳，最终状态和现场位置一致。 |
| 失败处理 | 规划失败、task ID 不一致、暂停/取消不生效、到点不稳或状态流身份错误时停止导航扩展，不重复发送目标。 |
| 证据 | `06-navigation-preview-goal.json`、规划路线、任务流、到点位置和现场视频。 |

建议命令：

```text
cargo run --example navigate -- https://<gateway>:5052 <x> <y> <z> <yaw> <timeout_seconds>
```

## 7. External 模式进入、逐关节目标、退出和安全回退

| 项目 | 内容 |
| --- | --- |
| 目标 | 证明客户策略可以通过公开 External API 接管关节目标，并在失败或退出时回到安全状态 |
| SDK 入口 | `motion().external_control()`、`enter()`、`enable()`、`publish_joints()`、`latest_reply()`、`disable()`、`close()`、`controller().state()` |
| 前置 | 前五个门通过，机器人处于稳定落地状态，策略输入经过现场审批，关节索引表与机型一致，专人持有急停。云端没有任何控制路径参与。 |
| 进入 | 建立 External 双向流，确认 native 仲裁进入外控并返回 controller state。先读取状态，再启用输出。 |
| 目标 | 发送低风险、受限幅度的逐关节目标。记录 sequence、joint index、accepted、错误和 native 回执。策略循环只在调用方控制的频率运行，不由 SDK 后台重发目标。 |
| 退出 | 先停止输入，再执行 disable，确认最终输出屏障，关闭 External 会话，读取 Builtin/External 当前模式和控制 owner。 |
| 通过标准 | 进入、启用、目标、禁用和关闭的 request identity 一致。错误、断流、超时和取消都触发安全回退。关闭后不再接受旧目标，控制 owner 和电机输出回到合同定义的安全状态。 |
| 失败处理 | 任一回执未知、索引不一致、目标被错误重放、输出未确认关闭或断线未回退时立即使用现场急停并标记 External 门失败。 |
| 证据 | `07-external-handover.json`、控制器状态流、逐关节回执、输出关闭确认、现场视频和急停记录。 |

## 结果汇总模板

| 门 | 结果 | 开始/结束 | 发布物与版本 | 证据目录 | 备注 |
| --- | --- | --- | --- | --- | --- |
| 1 System | `NOT RUN` |  |  |  |  |
| 2 Motion | `NOT RUN` |  |  |  |  |
| 3 Observations | `NOT RUN` |  |  |  |  |
| 4 Localization/Maps | `NOT RUN` |  |  |  |  |
| 5 Point cloud/Camera/Video | `NOT RUN` |  |  |  |  |
| 6 Navigation | `NOT RUN` |  |  |  |  |
| 7 External | `NOT RUN` |  |  |  |  |

## 现阶段状态

SDK 本地合同回归为 124 项通过。正式机器人 5052 服务、Brainstem 部署、真实传感器频率、动作、导航到点、视频、External 硬件接管和关闭回退仍需按本矩阵现场执行。未完成的门保持 `NOT RUN` 或 `BLOCKED`，不使用仿真结果替代真机结果。

## 2026-11-14 现场窗口记录

本窗口完成了真机只读前置和云传输层验证。`192.168.66.64` 曾连续 ping 成功，`brainstem.service` 为 active，`127.0.0.1:13146` 的 `GetControlStatus` 读回 `GROUNDED`、电机输出关闭、控制方 `NONE`、无严重电机故障、无 gRPC lease。四路 CAN 在前置时均为 `UP/ERROR-ACTIVE/1Mbps`，IMU 设备存在，authorizer 与 cloud-link 制品哈希通过大小写不敏感比较。

通过本机 authorizer API 开启只读云连接后，设备保存 `choice=true`，`doso-cloud-link` 进入 active，机器狗到 MQTT `8.153.169.177:8883` 的 TCP 连接为 `ESTABLISHED`，日志出现 `cloud_connected`。业务同步接口对该 UUID 返回 `permission:null`，因为研发库存仍未绑定账号，Gateway 没有向设备下发短期状态许可，因此 `POST /status` 和 `GET /status/latest` 仍为 `503 unavailable`。这证明传输层已通，不证明云端状态上报已通过。

设备随后从局域网消失，无法完成云许可下发后的 `status.report`、App 状态读取和断云回退。该窗口没有发送运动、遥操作、电机、导航或 External 命令，也没有修改 CAN。

| 门 | 当前结果 |
| --- | --- |
| 1 System | `BLOCKED`，13146 只读已通过，正式 5052/云状态投影待设备恢复并完成账号许可 |
| 2 Motion | `NOT RUN` |
| 3 Observations | `NOT RUN` |
| 4 Localization/Maps | `NOT RUN` |
| 5 Point cloud/Camera/Video | `NOT RUN` |
| 6 Navigation | `NOT RUN` |
| 7 External | `NOT RUN` |

证据保存在 `D:\DOSO\artifacts\robot-cloud\real-connect-20261113\field-preflight-sudo-20261114.json`、`read-current-13146.json`、`real-cloud-activation-20261114.json` 和 `real-cloud-observe-20261114.json`。

2026-11-15 开机复核再次通过 13146 只读读取，结果仍为 `GROUNDED`、电机输出关闭、控制方 `NONE`、无严重故障、无 gRPC lease。`choice=true` 已保留，cloud-link unit 已设置为 enabled/active 并重新建立 MQTT TCP。当前 CAN USB 路径与旧映射记录不同，四路 CAN 虽均为 `UP/ERROR-ACTIVE/1Mbps`，只读映射检查仍阻断后续维护窗口。设备账号尚未绑定，Gateway 没有 reporting permission，因此七域真机主体动作继续保持未运行。
