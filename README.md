# DOSO SDK

DOSO Rust SDK **0.2.0** 使用 **gRPC / Protobuf**。现场验收进度见 [TESTING.md](TESTING.md)。
当前本地控制和观测使用 `BrainstemClient` 直接连接机器狗 `13145`，不要求部署独立 `5052` 服务。七域 `DosoClient` 保留 LingTu 地图、导航和任务等高层能力的协议合同。
SDK 面向机器人本地能力，不持有云管理凭据。设备不能管理云端归属、授权或规则。运动 API 通过机器狗 Brainstem 的本地 gRPC 和机端控制权、安全链执行。云端只做设备登记、许可和只读状态同步，不发送运动命令，也不建立通用 gRPC 转发。
直接 Brainstem 连接读取 `control_status()`、`profile()`、`voltage()` 和 `motor_status()`。只有使用可选的七域 `DosoClient` 时才会执行 `system().info()` 协议握手。

直接 Brainstem 连接位于 [brainstem](src/brainstem/mod.rs)，控制状态、站坐、速度、External 逐关节接管和观测都沿用机器狗原生 gRPC 合同。七域连接实现位于 [client](src/client/mod.rs)，只用于可选的 LingTu 高层服务；与智身、宇树 Go2 的逐项差距见[连接与操控对照](docs/architecture.md#连接与操控对照)。

巡检路线 CRUD、任务执行/查询/订阅/控制与报告复用现有机器人任务 owner，接口见[tasks](docs/api/tasks.md#巡检路线与任务)。

客户自己的运控程序通过[外部运控模式](docs/api/motion.md#自定义控制器)持续提交逐关节目标；策略语言、推理框架和部署方式由客户选择。

高层地理位置使用 `localization().location()` 和 `watch_location()`，WGS84 经纬度与 SLAM 地图位姿分别提供，前提是已部署可选的 `5052` Gateway。直接 Brainstem 入口不包含 SLAM、地图或导航。[地理位置 API](docs/api/localization.md#地理位置)

```text
应用 → Rust SDK BrainstemClient → gRPC :13145 → Brainstem → CAN / 传感器 / 电机
```

## 开始使用

需要 Rust 1.94+ 和 `protoc`。获取 `v0.2.0`：

```text
git clone --branch v0.2.0 --single-branch https://github.com/Kitjesen/DOSO-SDK.git
cd DOSO-SDK
```

在仓库根目录执行：

```text
cargo build --locked
cargo run --example read_brainstem -- http://127.0.0.1:13145
```

远程 Brainstem 使用 `https://` 与设备签发的客户端证书，配置见 [直连 API](docs/api/brainstem.md)。应用可通过 Cargo Git 或本地 path 依赖接入。[快速入门](docs/usage.md) · [API 手册](docs/api/README.md)

导航提供地图目标及可选限速/到达半径、只读规划预览、运行中限速、任务状态/等待/控制与路线订阅。可运行的[导航到点示例](examples/navigate.rs)按一次目标关联同一个任务，使用前先查看 `cargo run --example navigate -- --help`。[导航 API](docs/api/navigation.md)说明当前公开能力与验收边界；规划与执行由机器人侧 LingTu 导航服务负责，正式机端服务部署和真实到点闭环仍待验收。

## 目录

```text
DOSO-SDK/
├── src/             Rust 七域实现、连接与错误处理
├── proto/           整机协议：领域 RPC 与 messages/ 消息格式
├── examples/        Rust 使用示例
├── tests/           Rust gRPC 合同测试
├── scripts/         协议绑定生成工具
├── docs/            架构、使用说明、计划与验证记录
├── brainstem/       Brainstem 协议、Rust 直连客户端与 C++ 底层客户端
├── archive/python/  历史 Python HTTP 预览
├── Cargo.toml       Rust 包入口
└── CMakeLists.txt   C++ 下游兼容构建入口
```

Rust 是当前主线。Brainstem C++ 客户端保留独立的[构建与安装说明](brainstem/README.md)；旧 Python 预览保留为[历史参考](archive/python/README.md)。

## 文档与验证

- [逐板块功能与验收清单](docs/features.md) · [测试记录与复现命令](TESTING.md)
- [API手册](docs/api/README.md)
- [对标后的能力缺口与推进顺序](docs/v1-plan.md#对标后的能力缺口)
- [架构与职责](docs/architecture.md) · [架构总览图（含云端）](docs/images/doso-sdk-architecture-cloud-20261107.png)
- [观测能力与数据来源](docs/observations.md)
- [SDK 消息格式与接入状态](docs/message-catalog.md)
- [相机图像和远程观看](docs/camera.md)
- [返回码与状态定义](docs/results.md)
- [机器人信息与固件发布](docs/release.md)
- [V1 产品计划](docs/v1-plan.md)
- [通信选择](docs/transport-selection.md)
- [直接 Brainstem 接入](docs/direct-brainstem.md)
- [gRPC 推进与验证记录](docs/grpc-v1-progress.md)

`cargo test --locked` 运行客户端测试和使用说明中的示例编译检查。
七域 Gateway 支持 API key 和 TLS。`BrainstemClient` 直连支持 mTLS，证书配置见 [Brainstem API](docs/api/brainstem.md)。设备动作与观测流长稳的真机结果见[测试记录](TESTING.md)。

推进记录中以 `D:/` 开头的链接是开发机上的历史测试证据，不随本仓库分发；API、协议、示例和构建说明使用仓库内链接。
