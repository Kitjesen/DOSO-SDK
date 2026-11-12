# DOSO SDK

DOSO 整机 SDK 使用 **Rust + gRPC/Protobuf**，当前处于开发预览阶段。
开发者通过一个 `DosoClient` 访问系统、运动、定位、地图、导航、观测和高层任务。
SDK 面向机器人本地能力，不持有云管理凭据。设备不能管理云端归属、授权或规则；云端禁止远程运动控制，本地 SDK 操控继续通过机端控制权和安全链。
连接完成协议握手后，通过 `system().info()` 取得固件版本、机器人版本和机器人 ID。健康与就绪状态单独查询。

连接实现位于 [client](src/client/mod.rs)，站坐、速度控制和停止位于 [motion](src/motion/mod.rs)，导航目标与任务控制位于 [navigation](src/navigation/mod.rs)。调用方式见[站起与坐下](docs/usage.md#站起与坐下)、[受控遥操作](docs/usage.md#受控遥操作)，与智身、宇树 Go2 的逐项差距见[连接与操控对照](docs/architecture.md#连接与操控对照)。

客户自己的运控程序通过[外部运控模式](docs/api/motion.md#自定义控制器)持续提交逐关节目标；策略语言、推理框架和部署方式由客户选择。

本地地理位置使用 `localization().location()` 和 `watch_location()`，WGS84 经纬度与 SLAM 地图位姿分别提供。1Hz 订阅保留 GNSS 源时间与可用状态，示例为 `cargo run --example watch_location -- http://<robot>:5052`。[地理位置 API](docs/api/localization.md#地理位置)

```text
应用 → Rust SDK → gRPC :5052 → LingTu 领域操作 → 现有机器人服务
```

## 开始使用

需要 Rust 1.94+ 和 `protoc`。Rust 主线位于 `rust` 分支：

```text
git clone --branch rust --single-branch https://github.com/Kitjesen/DOSO-SDK.git
cd DOSO-SDK
```

在仓库根目录执行：

```text
cargo build --locked
cargo run --example read_status -- --help
```

应用可通过 Cargo 的 Git 依赖引用 `rust` 分支，也可使用本地 path 依赖。安装与常用流程见[快速入门](docs/usage.md)，函数签名、参数、返回值和示例见[API手册](docs/api/README.md)。

导航已提供地图目标提交、任务状态、等待终态、暂停、恢复、取消与路线订阅。可运行的[导航到点示例](examples/navigate.rs)按一次目标关联同一个任务，使用前先查看 `cargo run --example navigate -- --help`。[导航 API](docs/api/navigation.md)说明当前公开能力与验收边界；规划与执行由机器人侧 LingTu 导航服务负责，正式机端服务部署和真实到点闭环仍待验收。

## 目录

```text
DOSO-SDK/
├── src/             Rust 七域实现、连接与错误处理
├── proto/           整机协议：领域 RPC 与 messages/ 消息格式
├── examples/        Rust 使用示例
├── tests/           Rust gRPC 合同测试
├── scripts/         协议绑定生成工具
├── docs/            架构、使用说明、计划与验证记录
├── brainstem/       仍在使用的 C++ 底层客户端
├── archive/python/  历史 Python HTTP 预览
├── Cargo.toml       Rust 包入口
└── CMakeLists.txt   C++ 下游兼容构建入口
```

Rust 是当前主线。Brainstem C++ 客户端保留独立的[构建与安装说明](brainstem/README.md)；旧 Python 预览保留为[历史参考](archive/python/README.md)。

## 文档与验证

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
- [gRPC 推进与验证记录](docs/grpc-v1-progress.md)

`cargo test --locked` 运行客户端测试和使用说明中的示例编译检查。
七域接口已接入本地 gRPC 服务，已提供站起/坐下请求、受控遥操作双向流、50Hz任务运行快照流、类型化定位状态，以及 IMU、关节、相机和点云订阅。姿态链原生构建与本地合同验证已通过，设备动作待现场验收，BMS、足端等真实数据源仍待接入。当前方法与来源边界见使用说明。

gRPC 服务已支持 API key 鉴权和 TLS，客户端支持私有 CA。未配置凭据和证书时采用开发用明文 listener。已有测试证据、仿真与实机边界见推进记录，本库尚未正式发布。

推进记录中以 `D:/` 开头的链接是开发机上的历史测试证据，不随本仓库分发；API、协议、示例和构建说明使用仓库内链接。
