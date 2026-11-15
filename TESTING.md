# SDK 测试记录

更新：2026-11-15。功能和缺口集中在[逐板块清单](docs/features.md)，本页记录测试范围、结果和复现方法。API 文档与消息目录不各自维护一份测试总数。七域真机验收按[七域真机验收矩阵](docs/REAL_ROBOT_ACCEPTANCE_MATRIX.md)执行。

## 验证层级

| 层级 | 证明什么 | 不证明什么 |
| --- | --- | --- |
| Rust 本地 gRPC 合同 | 真实 Tonic TCP/流、参数、身份、错误、超时及客户端行为 | native 算法、设备输出、机端部署 |
| Gateway/设备源本地测试 | 实际 handler、共享事务和适配器对测试源的映射 | 实际传感器/执行器工作、真实路线可行 |
| 构建/格式/Clippy/打包 | 支持目标上的编译、静态规则和包内容 | 实时性能、机器人行为 |
| 仿真闭环 | 指定场景中地图/导航/任务/停止行为 | 真实地形、CAN、电机和现场网络 |
| 真机 | 明确机器、发布物、链路和观察窗口中的结果 | 其他版本、其他链路或更长时间稳定性 |

## 本轮结果

首轮 Rust gRPC **95 项通过**，包含显式重连 5 项、导航增量 4 项、相机内参 2 项。之后新增巡检与旧导航服务兼容修正，最终完整回归结果见下表。

| 检查 | 结果 | 证据范围 |
| --- | --- | --- |
| 导航 Gateway + 原七域 gRPC | 70 passed | `test_grpc_navigation_options.py` + `test_grpc_sdk.py`；包含本次 17 项导航参数/预览/限速检查 |
| 相机 Gateway + 原相机 gRPC + DDS/Orbbec | 61 passed | 新标定读取、媒体鉴权、缺来源、4 个直角旋转几何用例；实际 SHM info 经默认 DDS source 发布到 gRPC |
| 巡检最终专项 | 75 passed，28.42秒 | 11项gRPC（含留存列表）+64项原HTTP；路线/会话/地图/journal/录制回滚，和323项有重叠不重复累计 |
| SDK 最终完整回归 | 124 passed | 8 单元 + 103 真实本地gRPC + 2消息格式 + 11使用文档测试；已含巡检任务列表 |
| 示例 / Android host | 7 / 7 passed | 示例参数与测量逻辑；Android独立workspace的host bridge |
| Gateway/源统一回归 | 323 passed，61.81秒 | 上述15个文件，无剩余失败；随后列表增量由75项巡检专项覆盖，不重复累计 |
| 协议生成 | 2 passed | 重复生成、导入、公开类型与清除旧绑定 |
| 格式 / Clippy / 文档链接 | 通过 | Rust全部目标Clippy；333个当前本地文档链接 |
| 打包与解包编译 | 通过，139文件 | crates.io索引超时后 `--offline -j 1` 成功；包含 TESTING.md |
| 真机相机 SDK 单帧（临时 gRPC 桥） | 通过（relay 预热后） | `Observations.CameraFrame` 从真实 go2rtc JPEG relay 返回 17,837 bytes，JPEG 头尾有效；不是正式 5052 Gateway |
| 真机相机 SDK 连续流（临时 gRPC 桥） | 通过（relay 预热后） | `Observations.WatchCameraSamples` 收到 10/10，序号 1–10，唯一帧 10/10；冷启动曾出现 5 秒零字节超时 |
| App/WHEP 代码回归 | 60 passed | `cloud-video`、`native-video-acceptance`、`whep-receiver`、surface/viewer tests；不等于正式 App 控制页播放 |
| App 设备发现/绑定回归 | 76 passed | QR、设备归属、直接机器狗连接和本地多设备 PostgreSQL 合同；不绑定真实账号 |
| App 原生点云编译检查 | 1 passed | JBR 21 + Android SDK，真实 Expo modules compile jar；不等于真机点云帧验收 |
| 云端媒体合同回归 | 9 passed | `doso-cloud-media` 会话/授权/过期/释放；使用本地上游替身，不等于公网媒体 |

中间失败也保留：一次导航准入测试复用了成功 request_id，正确命中既有幂等 journal，测试改为新请求后通过；并行开发期间相机测试两次在收集阶段遇到巡检绑定导入错误，修正包路径/重新生成后 48 项通过。巡检新增测试也曾因夹具使用错误的环境错误码和急停状态结构出现2项失败；修正为现有owner实际合同后，窄测10项及统一323项通过。以上均不是现场传感器故障。最终完整 Rust 构建曾因 Windows 页面文件不足（os error 1455）和并行 rustc 内存分配失败中断；采用 `-j 1` 重跑，结果单独记录，不把资源失败改写成接口失败或直接忽略。

Linux/Windows 的当前提交 CI 结果由 [GitHub Actions](https://github.com/Kitjesen/DOSO-SDK/actions?query=branch%3Arust) 的对应提交记录；本地 Windows 结果不冒充 Linux 验收。

## 按板块检查

| 板块 | 已有本地检查入口 | 尚未验收 |
| --- | --- | --- |
| client/system | [grpc.rs](tests/grpc.rs)、[client](tests/grpc/client.rs)、[product](tests/grpc/product.rs)、[TLS](tests/grpc/tls.rs)、[control](tests/grpc/control.rs) | 正式机端三项身份、证书/ACL、重启后握手与原事务补查 |
| motion | [posture](tests/grpc/posture.rs)、[motors](tests/grpc/motors.rs)、[profile](tests/grpc/profile.rs)、[teleop](tests/grpc/teleop_publish.rs)、[取消](tests/grpc/teleop_cancel.rs)、[external](tests/grpc/external.rs)、[kinematics](tests/grpc/kinematics.rs) | 站坐、中途停止、电机门、50Hz真机遥操作、抢占/断线物理停止、External硬件接管 |
| localization | [location](tests/grpc/location.rs)、[task/state](tests/grpc/task_watch.rs)、[grpc.rs](tests/grpc.rs) | 真机 GNSS、源时间、真实地图定位与重定位 |
| maps | [grpc.rs](tests/grpc.rs) 的地图/PCD/栅格/事务测试；Gateway 的 `test_grpc_map_content.py`、`test_grpc_map_grid.py` | mapd 实际落盘、加载与 Product 切换后补查、实机栅格/PCD |
| navigation | [navigation](tests/grpc/navigation.rs)、[grpc.rs](tests/grpc.rs) 的任务/路线测试；Gateway 的 `test_grpc_navigation_options.py` | native 实际成路、真实到点、限速效果、暂停/取消停稳 |
| observations | [observation_metadata](tests/grpc/observation_metadata.rs)、[formats](tests/message_formats.rs)、[grpc.rs](tests/grpc.rs) 的源事件/相机/点云；`examples/read_camera.rs`、`examples/watch_camera_samples.rs` | 15分钟并发长稳、加速度来源、当前部署关节索引、正式 5052 相机/内参、点云、手机渲染 |
| tasks | [inspection](tests/grpc/inspection.rs)、[task_watch](tests/grpc/task_watch.rs)；Gateway 的 `test_grpc_inspection.py` 与原 HTTP 巡检回归 | 真机巡检轨迹/证据/报告和暂停恢复；统一跟随/Agent 历史尚无完整接口 |

Gateway 测试文件位于相邻 `lingtu` 仓库，Rust 文件位于本仓库。原生服务未实际启动时，不能把测试替身的 ACK、路线或终态写为 native/真机验收。

## 配套机端代码

本轮 Rust SDK 需要配套 LingTu handler 才能使用新增方法。旧服务未实现新 RPC 时明确返回 UNIMPLEMENTED；带限制的目标不降级为不带限制的运动。

| 机端职责 | LingTu 仓库内位置 |
| --- | --- |
| 新增领域 gRPC 接线与流 | `src/gateway/rpc/services.py`、`navigation.py`、`navigation_control.py`、`observations.py`、`inspection.py`、`task_run.py` |
| 原巡检事务共用 | `src/gateway/routes/inspection.py` 的模块级操作；HTTP包装和gRPC复用 |
| 默认DDS与进程内相机来源 | `src/drivers/real/camera/dds_module.py`、`module.py` |
| 生成的Python消息 | `src/gateway/rpc/generated/`，由本SDK脚本生成 |

这些机端改动位于相邻 LingTu 共享工作区，本轮未部署到机器人，也没有把该仓库中其他人的改动一起提交。SDK Git/Cargo 包不能自行安装或启动机器人服务。

## 复现命令

Windows、Linux 的 Rust 命令相同，需要 Rust 1.94+ 和 `protoc`：

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked -j 1
cargo test --workspace --examples --locked -j 1
cargo test --manifest-path android/Cargo.toml --locked -j 1
cargo package -p doso-sdk --allow-dirty --locked -j 1
```

构建机内存紧张时保留 `-j 1`；crates.io 索引不可达但锁定依赖已缓存时，打包命令可加 `--offline`。

Android 是独立 workspace，根目录 `--workspace` 不包含它。文档测试由 `src/lib.rs` 引入 `docs/usage.md`；不宣称自动编译了每个 API Markdown 页的所有示例。host bridge 通过也不表示 Android 目标交叉编译或 APK 验收。

协议生成使用同一套 proto，勿手改生成物。在 SDK 根目录、安装了 `grpcio-tools` 的 Python 环境执行：

```text
python scripts/generate_grpc.py --lingtu ../lingtu
python -m unittest discover -s scripts -p test_generate_grpc.py -v
```

本机该生成环境是 `C:/Users/99563/miniconda3/python.exe`；LingTu `.venv` 没有 `grpc_tools`，生成器测试必须使用前者。未为此修改共享虚拟环境。

在 LingTu 仓库使用已有 `.venv` 运行受影响 Python 检查：

```text
python -m pytest tests/gateway/test_grpc_sdk.py tests/gateway/test_grpc_navigation_options.py tests/gateway/test_grpc_inspection.py tests/gateway/test_grpc_observation_metadata.py tests/gateway/test_camera_grpc.py tests/gateway/test_live_cloud_grpc.py tests/gateway/test_grpc_security.py tests/gateway/test_grpc_lifecycle.py tests/gateway/test_grpc_map_content.py tests/gateway/test_grpc_map_grid.py tests/gateway/test_rpc_telemetry.py tests/gateway/test_rpc_kinematics.py tests/gateway/test_gateway_inspection_routes.py tests/drivers/test_orbbec_camera_module.py tests/drivers/test_camera_dds_module.py -q
```

DDS 相机源由 `tests/drivers/test_camera_dds_module.py` 和观测元数据测试覆盖。先生成协议再运行服务测试；旧生成物缺少新 RPC 时产生的 collection/trait 错误不能当作通过。

只读实机测量入口：

```text
cargo run --example measure_observations -- --help
cargo run --example measure_observations -- http://<正式Gateway>:5052 900 both
```

第二条需要正式已部署、可访问的 Gateway；本轮没有执行。示例连续 2 秒未消费 IMU 或任一关节即失败，不自动重连掩盖停流，详细统计语义见[观测 API](docs/api/observations.md#固定窗口测量实际接收频率)。

## 现场记录

以下是 **2026-11-12 的历史观察**，不是 11-13 的当前状态检查。本轮没有重启服务、切换运控、使能电机或发送动作。

- THUNDER-DEV-001，`192.168.66.64`，UUID `d82a831a-2e86-4b46-975b-3d42473583b7`。22:55:56 CST 只读检查 Brainstem active，13145 在线，13146/5052 未监听；Host/云连接未运行。不能据此说正式机端 SDK 已安装。
- `current-concurrent-20261112T145643Z.json`：经计算机观察桥，SDK 连续 60.001253 秒完成；IMU 6001 条、100.015Hz、最大消费间隔 104.293ms；16 轴各约 99.931–99.965Hz、最大单轴间隔 108.582ms。原始旁路也完成。握手三项身份在该桥均为 None。
- 这次 60 秒完成不抵消此前失败，也不证明 15 分钟稳定、同步采样、端到端延迟或机器人本机 Gateway 性能。
- 历史约 200.319 秒停流的根因仍未证明；约 316 秒 GOAWAY(11) 已定位为 grpc-dart 5.2 keepalive 路径；另有服务冲突引发 shutdown 超时，以及 USB CAN 断开/重枚举 DOWN 的独立事件。它们不能合并成一个 SDK 原因。
- keepalive、loopback 13146、read-stream close 候选已完成各自离线验证和 ARM64 构建，但没有在本轮激活；不能用候选测试替代运行服务的验收。
- 2026-11-15 真机只读电机读取通过：`GetMotorStatus` 返回 16/16 在线，离线列表、非零状态码和错误列表均为空，报告温度 26–28°C。该接口没有样本时间和有效位，不能由一次读取推导长期健康。
- 同日 900 秒 IMU/关节并发窗口实际运行约 314 秒后收到 `GOAWAY error 11`。原始直连 IMU 99.923Hz，16 个关节约 98.87–98.90Hz；Rust SDK 观测桥在同一连接结束前统计到约 46Hz，窗口未完成。该结果保留为当前真机稳定性失败，不能写成 15 分钟通过。测试工具启动等待已从 5 秒扩大为 30 秒，避免把观测桥启动慢误判为采样失败。[并发记录](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/current-concurrent-20261114T163345Z.json) · [SDK日志](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/current-sdk-20261114T163345Z.log) · [原始流](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/raw-bdp-on-900s-20261114T163409Z.json)
- 同日冷启动相机只读检查显示 LingTu Orbbec 原生相机为 `streaming`，约 `29.7 FPS`，但 `/api/v1/camera/snapshot` 曾返回 `503` 或超时，5052 正式 Gateway 未监听。[冷启动检查](D:/DOSO/artifacts/robot-cloud/real-connect-20261113/camera-preflight-20261115.json)

完整原始现场文件位于开发机 `D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/`，不随 SDK Git 包分发；[历史推进记录](docs/grpc-v1-progress.md)继续保留早期结果。

## 2026-11-15 视频现场窗口

机器狗 `192.168.66.64` 的 Orbbec 源、RTSP `8554/cam` 和 go2rtc JPEG relay 均可产出真实图像。用临时只读 gRPC 桥读取真实 relay 后，Rust `read_camera` 成功保存 17,837 bytes，头尾为 `FFD8`/`FFD9`；`watch_camera_samples` 收到 10 帧，序号连续 `1..10`、唯一帧 `10/10`。原始证据为 [`sdk-camera-acceptance-20261115-go2rtc.json`](D:/DOSO/artifacts/robot-cloud/real-connect-20261113/sdk-camera-acceptance-20261115-go2rtc.json)。此前冷启动请求曾 5 秒零字节超时；真实 WHEP 预热后连续请求恢复为 HTTP 200、约 0.12–0.99 秒，因此 relay 仍需补明确的冷启动/就绪处理。

临时桥已停止并清理，机器狗没有留下 `50052`；正式 `5052` 未监听。因此这证明 SDK JPEG 消息和连续流能消费真实 relay，不证明正式 Gateway 部署，也不掩盖 relay 冷启动时序问题或 App/公网视频尚未交付。

App 侧 `cloud-video`、`native-video-acceptance`、`whep-receiver`、视频 surface/viewer 回归共 60 项通过（[`app-video-contract-regression-20261115T0431Z.json`](D:/DOSO/artifacts/robot-cloud/real-connect-20261113/app-video-contract-regression-20261115T0431Z.json)）；这些是代码/合同验收。局域网 WebRTC/WHEP 浏览器接收器已有真实通过证据（见 robot-cloud `real-webrtc-acceptance-20261115.json`），手机 App 真机显示仍未验收。对公网候选 `media-test.dosorobotics.com` 的只读 status/WHEP 请求均为 404，证据为 `D:/DOSO/artifacts/robot-cloud/real-connect-20261113/public-whep-probe-20261115.json`，因此公网 WHEP 尚未部署。后续只读检查观察到机器人的 go2rtc/相机/视频服务有较新的启动时间；本段记录的历史窗口没有发起重启，服务生命周期不作为 SDK 通过依据。

## 2026-11-15 当前只读复核

随后在同一台机器狗上重新做了无副作用复核：5050 相机状态为 `200/streaming`，Orbbec 约 `29.6 FPS`；在 LAN WHEP 观看者预热后，连续 5 次 `/api/v1/camera/snapshot` 均为有效 JPEG（25,277–25,600 bytes，5 个 SHA-256 均不同）。原始窗口见 [`camera-warm-readonly-20261115T013709Z.json`](D:/DOSO/artifacts/robot-cloud/real-connect-20261113/camera-warm-readonly-20261115T013709Z.json)。这把“预热后 relay 可读”从历史记录变成了当前窗口证据，但不改变正式 SDK 门：现场 `brainstem.service` 当前为 inactive，13146/5052 未监听，5050 仍是独立 video probe；正式 5052、App 真机显示和公网 WHEP 仍未通过。

## 2026-11-15 视频 producer 修复后复核

新鲜现场复核发现 go2rtc 的 `cam` producer 配置曾为空，恢复后旧配置又引用机端不存在的 `/usr/local/bin/lingtu_camera_webrtc`。已在机器狗保留两份配置备份，并只将 producer 切换到已安装的 `/opt/lingtu/current/bin/lingtu_camera_go2rtc_mpegts.sh`，重启 `go2rtc.service`；没有改 Brainstem、CAN、电机、运动命令或云连接。修复后 LAN WHEP 返回 201，Edge 真实解码为 640×480、首帧约 6.2 秒，10 个连续采样点增长；5 次相机快照均为完整且不同的 JPEG（25,941–26,204 bytes）。证据见 [`video-config-repair-20261115.json`](D:/DOSO/artifacts/robot-cloud/real-connect-20261113/video-config-repair-20261115.json)、[`camera-warm-current-20261115T0429Z.json`](D:/DOSO/artifacts/robot-cloud/real-connect-20261113/camera-warm-current-20261115T0429Z.json) 和 [`real-webrtc-acceptance-20261115.json`](D:/DOSO/artifacts/robot-cloud/real-connect-20261113/real-webrtc-acceptance-20261115.json)。

该复核提升的是“真机局域网媒体源/查看器”结果；Rust SDK 仍只能通过此前的临时只读 gRPC bridge 记为 `PASS_PARTIAL`，正式 SDK Gateway `5052` 尚未监听，因此没有把这次视频服务修复写成正式 SDK 单帧/连续流通过。

现场随后只读确认 `lt-host.service` inactive、`5052` 未监听，且缺少 `lt-host` 与 ProductControl session/env 输入；见 [`formal-gateway-live-check-20261115.json`](D:/DOSO/artifacts/robot-cloud/real-connect-20261113/formal-gateway-live-check-20261115.json)。因此正式 SDK 单帧/连续流的下一步不是新增 Rust API，而是完成匹配版本的 native release、TLS/API key 和 session，再用现有 `read_camera`/`watch_camera_samples` 示例重跑。

媒体清理边界也已记录：go2rtc `consumers=[]` 不等于 exec producer 已退出；一次现场窗口遗留的 root encoder 已精确清理，正式 Gateway/媒体部署需把 producer 回收纳入验收。[清理证据](D:/DOSO/artifacts/robot-cloud/real-connect-20261113/video-producer-cleanup-20261115.json)

同一时刻的 Brainstem 只读故障快照显示 `thunder-dualpose-can.service=failed`，四路 CAN 均为 `DOWN`，实际 USB 父路径与 unit 声明不一致（`can0` 实际 `1-2.1`，声明 `1-1.2`）。`can-check.py --check` 因此拒绝配置并退出；没有执行 `ip link`、重启或映射修正。[故障快照](D:/DOSO/artifacts/robot-cloud/real-connect-20261113/brainstem-can-readonly-20261115T014429Z.json)

隔离 Android 视频接收器随后完成真实 LAN WHEP 验收：当前源码构建的 x86_64 `com.doso.robot.videotest` 读取 `192.168.66.64:5050` 后进入 `视频：receiving`，原生 `whep-native-player` 节点存在，日志有 `setRemoteDescription`、`ontrack` 和持续 `getStats`；停止后当前会话释放。该窗口同时暴露并修复了 App 对 go2rtc `201` 无 `Location` 响应的兼容边界：仅 `backend=go2rtc` 接受此响应，native WHEP 仍要求资源地址。原始证据为 [`app-video-isolated-acceptance-20261115T031720Z.json`](D:/DOSO/artifacts/robot-cloud/real-connect-20261113/app-video-isolated-acceptance-20261115T031720Z.json)，代码回归见 App `whep-receiver.test.mjs`（34 项通过）。这仍不是正式 5052 或主 App 控制页验收。

## 未执行和已知限制

- 本轮未运行完整仿真、真实地图导航、机器人动作、External策略、手机/Android实装或真实公网视频。
- 没有运行全部 LingTu/native/Brainstem 仓库测试或重新构建整机固件；只检查本次受影响入口，避免触碰现场运行时。
- BMS、足接触、光照、外设与充电桩缺真实适配器；姿态/步态、Recovery/阻尼缺完整执行链，详见[未完成能力](docs/features.md)。
- 相机源内参的旋转畸变系数沿用原实现；标定数值、坐标轴及其与 JPEG 预览的几何对齐仍须实机验收。
- 单关节 gRPC 统一索引已在当前 Brainstem 源码核对，现场 flowfix10 是否包含归一化尚未确认；不能把旧事件 ID 直接当 External 目标索引。
