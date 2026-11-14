# SDK 测试记录

更新：2026-11-14。功能和缺口集中在[逐板块清单](docs/features.md)，本页记录测试范围、结果和复现方法。API 文档与消息目录不各自维护一份测试总数。七域真机验收按[七域真机验收矩阵](docs/REAL_ROBOT_ACCEPTANCE_MATRIX.md)执行。

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
| observations | [observation_metadata](tests/grpc/observation_metadata.rs)、[formats](tests/message_formats.rs)、[grpc.rs](tests/grpc.rs) 的源事件/相机/点云；Gateway 相机/实时点云测试 | 15分钟并发长稳、加速度来源、当前部署关节索引、真机相机/内参、点云、手机渲染 |
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

完整原始现场文件位于开发机 `D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/`，不随 SDK Git 包分发；[历史推进记录](docs/grpc-v1-progress.md)继续保留早期结果。

## 未执行和已知限制

- 本轮未运行完整仿真、真实地图导航、机器人动作、External策略、手机/Android实装或真实公网视频。
- 没有运行全部 LingTu/native/Brainstem 仓库测试或重新构建整机固件；只检查本次受影响入口，避免触碰现场运行时。
- BMS、足接触、光照、外设与充电桩缺真实适配器；姿态/步态、Recovery/阻尼缺完整执行链，详见[未完成能力](docs/features.md)。
- 相机源内参的旋转畸变系数沿用原实现；标定数值、坐标轴及其与 JPEG 预览的几何对齐仍须实机验收。
- 单关节 gRPC 统一索引已在当前 Brainstem 源码核对，现场 flowfix10 是否包含归一化尚未确认；不能把旧事件 ID 直接当 External 目标索引。
