# DOSO SDK 七域真机验收矩阵

更新日期：2026-11-19。本文记录 `rust` 分支的真实机器人验收动作、标准和结果。

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
| Brainstem | `192.168.66.64:13145`，使用 Rust `BrainstemClient` 直连 |
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
| 目标 | 证明 SDK 能通过 Brainstem 主 gRPC 服务读取底层状态和型号能力 |
| SDK 入口 | `BrainstemClient::connect`、`control_status()`、`profile()`、`voltage()`、`motor_status()` |
| 执行 | 使用 `13145` 直连，读取控制状态、Profile、电压和电机状态。记录身体状态、输出门、控制方、租约、故障、Profile 名称、16 路电压和 16 路电机在线状态。 |
| 通过标准 | 连接建立成功，身体状态字段存在，输出门和控制权状态可读，Profile 非空，电压和电机数组均为 16 项，读取过程不申请控制权。 |
| 失败处理 | 认证、TLS、协议版本或身份不一致时停止，不进入运动门。 |
| 证据 | `01-system-connect.json`、`01-system-connect.log`、版本与证书摘要。 |

建议命令：

```text
cargo run --example read_brainstem -- http://<robot>:13145
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

## 3. IMU、关节 100Hz、状态查询和电压

| 项目 | 内容 |
| --- | --- |
| 目标 | 验收 Brainstem 观测流的连续性和并发状态查询的完整性 |
| SDK 入口 | `BrainstemClient::listen_imu()`、`listen_joint()`、`control_status()`、`profile()`、`voltage()`、`motor_status()` |
| 基线窗口 | 连续 900 秒并行读取 IMU 和 16 轴关节，每 10 秒读取控制状态、Profile、电压和电机状态。验收 90 轮、360 次调用。15 分钟通过后建议扩展到 3,600 秒、360 轮、1,440 次调用。 |
| 通过标准 | 完整跑满窗口，工具报告 `passed=true` 并以 `0` 退出。IMU 和每个轴平均频率均为 98–102 Hz。SDK 接收空窗和机端消息时间戳间隔分别统计，各路最大值均不超过 2 秒。每轮四项查询成功、16 路电压均为有限非零值且读到 16 个在线电机。 |
| 抖动观测 | 保留 `imu` 和 `joint_0` 至 `joint_15` 的最大接收空窗，标出超过 200 ms 的关节轴。200 ms 用于观察波动，2 秒用于判定。机端消息时间戳用于衡量机端消息间隔。 |
| 判定 | `13145` 无监听或无法建立连接时记为 `BLOCKED`。窗口开始后提前结束、查询失败、缺轴、机端时间戳异常或未达阈值时记为 `FAIL`。纯流窗口 `900 0` 用于定位并发影响。 |
| 证据 | 记录命令、SDK 版本或提交、设备和机端 boot ID、Brainstem 构建版本、开始及结束时间、原始标准输出和错误输出、退出码。保留逐轴样本数、频率、两类最大间隔和查询摘要。 |

HTTPS 端点与证书按现场配置使用。[证书配置](api/brainstem.md)

```powershell
cargo run --locked --release --example measure_brainstem -- https://thunder-01.local:13145 900 10
cargo run --locked --release --example measure_brainstem -- https://thunder-01.local:13145 3600 10
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
| 视频执行 | 使用正式观看授权，通过 App 或批准的查看器确认画面持续解码。MQTT 只验证源启停消息，媒体帧走视频通道。临时 SDK 桥只用于隔离验证消息合同，不能替代正式 5052。 |
| 通过标准 | JPEG 可解码，尺寸和内参字段一致。点云帧可解码，坐标系和代次保留，空帧与 reset 有明确语义。取消后流结束且资源释放。视频有持续帧增长、停止观看后源按合同退出。 |
| 失败处理 | 权限错误、帧停流、元数据缺失、点云解码失败或取消不释放时停止该门，不把地图 PCD 当实时点云。 |
| 证据 | `05-camera-pointcloud-video.json`、`D:\DOSO\artifacts\robot-cloud\real-connect-20261113\sdk-camera-acceptance-20261115-go2rtc.json`、JPEG/点云摘要、视频观看记录、授权和取消日志。 |

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
| 3 Observations | `FAIL`。并发 900 秒，IMU / 最差关节轴接收空窗 8.747 / 4.696 秒；纯流 900 秒，两者均为 31.456 秒。360 次并发查询成功。 | 2026-11-19 20:34–21:05 CST | `v0.2.0` / `3265d81` | [两轮真机报告](D:/DOSO/artifacts/robot-cloud/sdk-soak-20261119T201433/RESULT.md) | 2 秒连续交付门槛未通过；1 小时验收待执行 |
| 4 Localization/Maps | `NOT RUN` |  |  |  |  |
| 5 Point cloud/Camera/Video | `FAIL`（relay 预热后相机临时桥通过；正式 5052、点云、App、公网未通过，冷启动不稳定） | 2026-11-15 | SDK 2bf587b | `D:\DOSO\artifacts\robot-cloud\real-connect-20261113` | 见相机窗口记录 |
| 6 Navigation | `NOT RUN` |  |  |  |  |
| 7 External | `NOT RUN` |  |  |  |  |

## 现阶段状态

SDK 本地合同回归为 124 项通过。当前 Brainstem 直连观测门已经执行并记为 `FAIL`，原因是 900 秒窗口存在超过 2 秒的长时间空窗，平均频率不能替代连续性。正式机器人 5052 服务不属于当前直接路径。动作、导航到点、External 硬件接管和关闭回退仍需按本矩阵现场执行。相机消息合同已通过真实 go2rtc relay 的临时桥隔离验收，但需要预热，Gate 5 仍为 `FAIL`；其他未完成的门保持 `NOT RUN` 或 `BLOCKED`，不使用仿真结果替代真机结果。

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

同一窗口的 900 秒 IMU/关节并发测量在约 314 秒收到 `GOAWAY error 11` 并结束。原始直连 IMU 平均 `99.923Hz`，16 个关节约 `98.87–98.90Hz`，但目标窗口没有完成。Rust SDK 观测桥在约 318 秒结束，桥接路径的关节统计约 `46Hz`。因此当前 Gate 3 记录为 `FAIL`，下一步先处理 Brainstem gRPC 长连接稳定性，再重做 900 秒窗口。原始证据为 `D:\DOSO\artifacts\robot-cloud\sdk-field-20261112\current-concurrent-20261114T163345Z.json`、`current-sdk-20261114T163345Z.log` 和 `raw-bdp-on-900s-20261114T163409Z.json`。

同日相机只读检查显示 LingTu Orbbec 原生相机服务为 `streaming`，状态接口报告约 `29.7 FPS`、彩色帧累计超过 28 万帧、最近帧约 442ms。真实 `/api/v1/camera/snapshot` 返回 `503` 或等待超时，正式 5052 SDK Gateway 仍未监听，尚未收到可解码 JPEG 或连续 SDK 相机流。因此 Gate 5 当前记录为 `FAIL`，相机源状态通过不等于 SDK 图像链路通过。证据为 `D:\DOSO\artifacts\robot-cloud\real-connect-20261113\camera-preflight-20261115.json`。

## 2026-11-15 视频 SDK 隔离窗口

本窗口只读验证相机，不绑定账号、不发送运动/电机/导航/External 命令。机器人 `192.168.66.64` 的 Orbbec 彩色/深度帧仍以约 30Hz 增长，真实 go2rtc `1984/api/frame.jpeg?src=cam` 在预热后返回 JPEG。临时桥将这一 relay 映射到 `Observations` 相机 RPC：Rust `read_camera` 单帧收到 17,837 bytes，JPEG 头尾有效；`watch_camera_samples` 收到 10/10 帧，序号 `1..10`、唯一帧 `10/10`。证据为 `D:\DOSO\artifacts\robot-cloud\real-connect-20261113\sdk-camera-acceptance-20261115-go2rtc.json`。

同一窗口冷启动时曾出现 go2rtc 请求 5 秒零字节超时；真实 WHEP 预热后连续请求恢复为 HTTP 200、约 0.12–0.99 秒，说明 relay 存在消费者/编码器就绪时序问题。临时桥已停止并清理，`50052` 与正式 `5052` 均不监听。因此本次通过只证明真实 relay 图像可被 SDK 消费和解码，不证明正式 Gateway 部署、冷启动就绪语义、手机 App 显示或公网 WHEP。局域网浏览器 WHEP 的独立通过证据仍为 `real-webrtc-acceptance-20261115.json`；公网和手机继续保持未验收。

随后复核确认：正式 `lt-host` 未安装，`5050` 仍由独立 `gateway.video_probe` 提供；真实 LAN WHEP 查看器可连接并在 10 秒内将 `framesDecoded` 从 55 推进到 325，之后 5 次 JPEG 请求全部成功。该结果将 Gate 5 保持为 `FAIL/PASS_PARTIAL`，而不是正式 `PASS`：冷启动 readiness、正式 `5052`、App 真机和公网媒体仍未满足。正式部署门与 gRPC 独立 API key/TLS 要求见 robot-cloud [FORMAL_GATEWAY_5052_DEPLOYMENT_20261115.md](D:/DOSO/robot-cloud/docs/FORMAL_GATEWAY_5052_DEPLOYMENT_20261115.md)。

## 2026-11-15 当前只读复核

当前窗口的真实只读结果为：`5050` 相机状态 `200/streaming`、Orbbec 约 `29.6 FPS`；LAN WHEP 观看者预热后，连续 5 次快照均返回完整 JPEG（25,277–25,600 bytes，5 个不同哈希）。证据为 [`camera-warm-readonly-20261115T013709Z.json`](D:/DOSO/artifacts/robot-cloud/real-connect-20261113/camera-warm-readonly-20261115T013709Z.json)。同一窗口再次确认 `brainstem.service=inactive`、13146 未监听、正式 5052 未监听，因此这些快照不能升级为 SDK 单帧通过；它们只证明预热后的独立 5050 relay 可读。冷启动 503/等待、正式 Gateway、App 真机和公网媒体仍是 Gate 5 的未完成项。

同窗口的机端故障快照确认 `thunder-dualpose-can.service` 因 USB-CAN 父路径与声明拓扑不一致而失败（`can0` 实际 `1-2.1`，unit 期望 `1-1.2`），四路接口均保持 `DOWN`。这解释了 Brainstem/13146 未启动，但不授权我们猜测映射或改动现场；后续维护窗口必须先由现场确认物理标签，再单独修正并重做 System 门。[只读快照](D:/DOSO/artifacts/robot-cloud/real-connect-20261113/brainstem-can-readonly-20261115T014429Z.json)

隔离 App 真机窗口补充通过：x86_64 `com.doso.robot.videotest` 读取独立 `5050`，状态进入 `视频：receiving`，原生 `whep-native-player` 挂载且 `getStats` 持续推进；点击停止后当前 WHEP 会话释放。该结果只把“App 接收器”标为 `PASS_ISOLATED_REAL_DEVICE`，不改变正式 `5052`、主 App 控制页、云端公网媒体和 SDK Gateway 的未通过状态。证据为 [`app-video-isolated-acceptance-20261115T031720Z.json`](D:/DOSO/artifacts/robot-cloud/real-connect-20261113/app-video-isolated-acceptance-20261115T031720Z.json)。

随后修复了真机 go2rtc 的实际 producer 配置：`streams.cam` 曾为空，旧配置恢复后又引用不存在的 `/usr/local/bin/lingtu_camera_webrtc`；已备份并切换到机端已安装的 `/opt/lingtu/current/bin/lingtu_camera_go2rtc_mpegts.py`，只重启 `go2rtc.service`。WHEP 重新返回 201，640×480 解码持续增长，0 丢包，查看器退出后 producer 子进程退出。该修复把独立 LAN 媒体源恢复为 `PASS_CURRENT_WINDOW`，但 Gate 5 仍为 `FAIL/PASS_PARTIAL`：正式 `5052`、正式 SDK 单帧/连续流、主 App 控制页和公网 WHEP 仍未通过。证据为 [`video-producer-pdeath-20261115.json`](D:/DOSO/artifacts/robot-cloud/real-connect-20261113/video-producer-pdeath-20261115.json) 和 [`camera-warm-current-20261115T0429Z.json`](D:/DOSO/artifacts/robot-cloud/real-connect-20261113/camera-warm-current-20261115T0429Z.json)。
