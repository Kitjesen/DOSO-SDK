# observations 观测与媒体

通过 `robot.observations()` 取得 `Observations<'_>`。`Result<T>` 为 `doso_sdk::Result<T>`，消息位于 `doso_sdk::rpc`。本领域读取设备和感知服务提供的数据，电机电压查询会发起底层只读 CAN 参数读取，相机订阅会登记观看者以支持按需编码。

入口与消息见 [Observations](../../src/observations/mod.rs)和[观测消息](../../proto/messages/observations.proto)。来源总览见[观测能力](../observations.md)，JPEG 接入细节见[相机说明](../camera.md)，点云消息边界见[消息清单](../message-catalog.md)。

## 方法索引

| 方法或类型 | 用途 |
| --- | --- |
| [scene](#scene) | 场景图快照 |
| [camera_status](#camera_status) | 相机服务诊断 |
| [camera_calibration](#camera_calibration) | 源彩色图像内参与畸变 |
| [camera_frame](#camera_frame) | 单次 JPEG 快照 |
| [watch_camera](#watch_camera) | 持续 JPEG 帧 |
| [camera_sample](#camera_sample) | 带可用元数据的 JPEG 快照 |
| [watch_camera_samples](#watch_camera_samples) | 带发布序号的 JPEG 流 |
| [watch_imu](#watch_imu) | IMU 源事件 |
| [watch_joints](#watch_joints) | 单关节或整帧关节源事件 |
| [motors](#motors) | 电机在线、温度和诊断 |
| [motor_voltages](#motor_voltages) | 电机母线电压 |
| [watch_point_cloud](#watch_point_cloud) | 扫描或地图显示点云流 |
| [CloudFrame decode](#cloudframe-decode) | 本地解码 PCLD v2 |
| [PointCloud 与 CloudStreamKind](#pointcloud-与-cloudstreamkind) | 解码数据与显示事件类型 |

## 公共调用与订阅约定

前置条件为已连接配套 Gateway，且目标数据源已装配并可访问。普通查询使用 `Config.timeout`，默认 10 秒。建立流返回 SDK `Error`，流内 `message().await` 返回 `tonic::Status`，`Ok(None)` 表示正常结束。流沿用连接 metadata，持续时间由调用方管理，丢弃流结束本地订阅并释放服务端资源。

长流与其他领域的订阅及地图下载共用 Gateway 流名额，当前最多 8 个。超额为 `RESOURCE_EXHAUSTED`，源缺失或失效可返回 `UNAVAILABLE`，旧服务缺少 RPC 时为 `UNIMPLEMENTED`。其他认证、连接与超时错误遵循[返回合同](../results.md)。频率、采样时间和缺失值按每种来源单独解释。

## 场景

### scene

```text
pub async fn scene(&self) -> Result<rpc::Snapshot>
```

无额外参数。调用 `Observations.Scene`，读取 Gateway 场景图缓存，经现有场景响应转换返回 `Snapshot.fields`。对象、关系、区域等扩展字段仍由场景服务维护，当前尚未形成单独的强类型场景对象 API。查询失败按公共约定返回，快照内时间字段按场景服务含义解释。

## 相机与 JPEG

相机方法的 `viewer_token: Option<&str>` 为媒体查看令牌。`None` 省略媒体授权头，`Some(token)` 发送 `authorization: Bearer ...`。空字符串或无法编码为 metadata 的值在本地返回输入错误。启用媒体保护时，缺少令牌为 `UNAUTHENTICATED`，错误令牌为 `PERMISSION_DENIED`，设备要求保护却未配置查看令牌为 `UNAVAILABLE`。连接的 `x-api-key` 鉴权与媒体查看权限分别处理，TLS 配置见[连接使用说明](../usage.md#api-key-与-tls)。

### camera_status

```text
pub async fn camera_status(&self) -> Result<rpc::Snapshot>
```

无额外参数。调用 `Observations.CameraStatus`，读取现有相机健康、装配与帧服务状态，返回诊断 `Snapshot`。该方法使用普通请求超时，失败按公共查询约定返回。

### camera_calibration

```text
pub async fn camera_calibration(
    &self,
    viewer_token: Option<&str>,
) -> Result<rpc::CameraCalibration>
```

调用 `Observations.CameraCalibration`，读取已装配相机源最近一次发布的彩色图像标定。沿用媒体查看鉴权与普通查询超时，不打开相机、不申请控制权，也不依赖 JPEG 观看者。真实环境默认 `camera_backend=dds` 的 `DdsCameraModule` 从原生相机 SHM info 帧缓存已发布标定；进程内 Orbbec 模块也通过同一 `get_camera_intrinsics()` 能力提供最近标定，Gateway 从已有模块注册表查询。尚未发布标定、没有这一能力或参数无效时返回 `UNAVAILABLE`；旧服务没有该 RPC 时返回 `UNIMPLEMENTED`。

返回 `width_px`、`height_px`、`fx_px`、`fy_px`、`cx_px`、`cy_px`、`frame_id`、`distortion_model` 和 `distortion`。尺寸与焦距/主点单位为像素，当前畸变模型 `plumb_bob`，系数顺序为 `[k1, k2, p1, p2, k3]`。字段直接来自源的 `CameraIntrinsics`，源安装旋转之后的参数由源负责；不虚构采样时间或设备序列号。

**这份标定对应源彩色图像。** JPEG 预览链还可能施加显示旋转等变换，不能直接把这些内参当成预览 JPEG 的标定。接口忠实发布源缓存；源目前旋转主点和尺寸时仍原样保留畸变系数，旋转后的畸变模型适用性尚未完成几何验收。`CameraSample.info` 继续保留其已有的未知字段语义。当前已测试本地 gRPC 数值、缺源、无效标定、鉴权、旧服务错误和四种直角旋转的像素主点公式；真机原始标定、旋转数值与 JPEG 几何对齐未验收。

### camera_frame

```text
pub async fn camera_frame(&self, viewer_token: Option<&str>) -> Result<rpc::CameraFrame>
```

调用 `Observations.CameraFrame`，复用机器人端现有 JPEG 快照来源。返回 `CameraFrame.jpeg`，内容为完整 JPEG 文件字节，单次快照允许使用已有缓存。

使用普通请求超时。数据源不可用返回 `UNAVAILABLE`，媒体授权错误按本节约定返回。单帧受客户端默认 4MiB gRPC 消息解码上限约束，超限可返回 `OUT_OF_RANGE`。

### watch_camera

```text
pub async fn watch_camera(
    &self,
    viewer_token: Option<&str>,
) -> Result<tonic::Streaming<rpc::CameraFrame>>
```

调用 `Observations.WatchCamera`，登记观看者后，等待订阅建立之后新发布的 JPEG 帧。Gateway 每 100ms 检查发布序号，序号变化时交付帧。实际频率取决于相机、编码和网络。

编码器冷启动与运行期间采用现有 5 秒新帧等待阈值，持续没有新发布时流以 `UNAVAILABLE` 结束。相机明确未装配或报告错误也可返回该码。取消流会释放观看者登记与流名额。新发布序号描述服务发布，硬件采样的新鲜度仍取决于源。

### camera_sample

```text
pub async fn camera_sample(&self, viewer_token: Option<&str>) -> Result<rpc::CameraSample>
```

调用 `Observations.CameraSample`，返回 JPEG 快照及可用元数据。请求超时、缓存和媒体授权规则与 `camera_frame()` 一致。

| 字段 | 当前来源与可选语义 |
| --- | --- |
| `frame` | `Option<CameraFrame>`，包含 JPEG |
| `info.encoding` | 当前已知为 `"jpeg"` |
| `info.camera_id`、`width_px`、`height_px`、`frame_id` | 设备未提供时保持 `None` |
| `time` | 当前没有源采样时间，保持缺失 |
| `sequence` | 快照回退路径当前保持缺失 |

### watch_camera_samples

```text
pub async fn watch_camera_samples(
    &self,
    viewer_token: Option<&str>,
) -> Result<tonic::Streaming<rpc::CameraSample>>
```

调用 `Observations.WatchCameraSamples`，沿用 `watch_camera()` 的发布、新帧等待、授权与资源释放规则，返回 `CameraSample`。`sequence` 为机器人服务 JPEG 发布计数，随服务重启重新计数。其余未知元数据保持缺失。序号适合观察发布进展，源采样时间由 `time` 的可用性单独表达。

仓库示例 [`examples/watch_camera_samples.rs`](../../examples/watch_camera_samples.rs) 可按指定数量读取帧，校验 JPEG 头尾并统计序号/唯一帧数。它只验证 SDK 消费合同；正式部署仍要求机器人 Gateway 提供可用的 JPEG relay，不能把 RTSP 临时桥当成生产端点。

以下示例覆盖相机查询与两种订阅形式，每种订阅读取一帧后显式结束。

```rust,no_run
use doso_sdk::DosoClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    let observations = robot.observations();
    let token = std::env::var("DOSO_MEDIA_VIEWER_TOKEN").ok();
    println!("{:?}", observations.camera_status().await?);
    let frame = observations.camera_frame(token.as_deref()).await?;
    println!("jpeg_bytes={}", frame.jpeg.len());
    println!("{:?}", observations.camera_sample(token.as_deref()).await?);
    let mut frames = observations.watch_camera(token.as_deref()).await?;
    if let Some(frame) = frames.message().await? {
        println!("jpeg_bytes={}", frame.jpeg.len());
    }
    drop(frames);
    let mut samples = observations.watch_camera_samples(token.as_deref()).await?;
    if let Some(sample) = samples.message().await? {
        println!("sequence={:?} info={:?}", sample.sequence, sample.info);
    }
    Ok(())
}
```

## IMU 与关节

### watch_imu

```text
pub async fn watch_imu(&self) -> Result<tonic::Streaming<rpc::ImuSample>>
```

无额外参数。调用 `Observations.WatchImu`，经机器人端只读适配器转发 Brainstem `ListenImu` 源事件。RunPlan 需要装配并接通相应观测源。SDK 当前按源事件交付，采样或重采样频率由源配置决定，接口没有选频参数。

| 字段 | 单位与含义 |
| --- | --- |
| `angular_velocity_rad_s` | 可缺省消息，三轴角速度 rad/s |
| `orientation_world_to_body` | 可缺省消息，world 到 body 的四元数，具名 w/x/y/z |
| `linear_acceleration_m_s2` | 可缺省消息，线加速度 m/s²，源未提供时为 `None` |
| `source_elapsed` | 可缺省的 Brainstem 服务相对时长，记录源事件映射时间 |

源事件缺少角速度或姿态时 Gateway 返回错误。源服务未配置或不可用返回 `UNAVAILABLE`，上游错误与流内取消按公共订阅约定传递。`source_elapsed` 适用于同一服务生命周期中的比较，硬件采样时钟仍需源提供明确合同。

### watch_joints

```text
pub async fn watch_joints(&self) -> Result<tonic::Streaming<rpc::JointSample>>
```

无额外参数。调用 `Observations.WatchJoints`，转发 Brainstem `ListenJoint` 单关节事件或完整关节帧，保留事件边界。

| 字段 | 单位与含义 |
| --- | --- |
| `joints[].id` | 源关节 ID，当前适配器要求 0–15 |
| `position_rad` | 位置，rad |
| `velocity_rad_s` | 速度，rad/s |
| `torque_nm` | 力矩，N·m |
| `status_code` | 源原始状态码 |
| `source_elapsed` | 可缺省的源服务相对时长 |

完整帧当前要求位置、速度、力矩和状态各有 16 项，单关节事件只交付该关节。型号的轴对应关系应按设备确认，跨事件的同步性由源定义。越界 ID、缺少关节数据或不完整源帧返回错误。未配置观测源、上游断连及流内错误按公共约定处理。

#### DOSO Thunder 16 轴编号

当前配套 Brainstem 源码在发布单关节 gRPC 事件前，将原始 CAN 报告编号转换为 `JointsMatrix` 索引；`JointSample.joints[].id`、整帧数组索引、`MotorState.id`、`MotorVoltages.volts` 索引与 External `targets` 索引因而一致。**老部署可能仍发送原始报告编号；SDK 保留上游 ID，不自行猜测转换。** 2026-11-12 的 flowfix10 真机部署尚未验证这项归一化，不能直接用旧部署的观测 ID 寻址 External 目标。

| gRPC ID / 电机诊断索引 / External target 索引 | 关节 | leg | CAN ID | 原始报告 ID |
| --- | --- | --- | --- | --- |
| 0 | 右前髋 `frHip` | 0 | 1 | 0 |
| 1 | 右前大腿 `frThigh` | 0 | 2 | 1 |
| 2 | 右前小腿 `frCalf` | 0 | 3 | 2 |
| 3 | 左前髋 `flHip` | 1 | 1 | 4 |
| 4 | 左前大腿 `flThigh` | 1 | 2 | 5 |
| 5 | 左前小腿 `flCalf` | 1 | 3 | 6 |
| 6 | 右后髋 `rrHip` | 2 | 1 | 8 |
| 7 | 右后大腿 `rrThigh` | 2 | 2 | 9 |
| 8 | 右后小腿 `rrCalf` | 2 | 3 | 10 |
| 9 | 左后髋 `rlHip` | 3 | 1 | 12 |
| 10 | 左后大腿 `rlThigh` | 3 | 2 | 13 |
| 11 | 左后小腿 `rlCalf` | 3 | 3 | 14 |
| 12 | 右前足/轮 `frFoot` | 0 | 4 | 3 |
| 13 | 左前足/轮 `flFoot` | 1 | 4 | 7 |
| 14 | 右后足/轮 `rrFoot` | 2 | 4 | 11 |
| 15 | 左后足/轮 `rlFoot` | 3 | 4 | 15 |

来源为 Brainstem `skinny_dog_algebra/lib/src/joints_matrix.dart` 的具名顺序、`thunder/lib/src/real_joint.dart` 的 `_flatIndexToLegCan()` 与 `sendJointCommand()`、`thunder/lib/src/server/proto_convert.dart` 的 `jointReportIdToMatrixIndex()`，以及 `thunder/bin/thunder.dart` 的发布入口。公式：原始 ID 为 `leg * 4 + CAN ID - 1`；矩阵索引在 CAN 1–3 时为 `leg * 3 + CAN ID - 1`，CAN 4 时为 `12 + leg`。这是当前 DOSO Thunder 型号合同，不是所有机器人通用关节清单；正方向、零位、齿比与限位仍以对应机器标定配置为准，不能从编号推导。当前没有跨型号关节描述 RPC。

```rust,no_run
use doso_sdk::DosoClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    let observations = robot.observations();
    let (mut imu, mut joints) = tokio::try_join!(
        observations.watch_imu(), observations.watch_joints()
    )?;
    let (imu_sample, joint_sample) = tokio::try_join!(imu.message(), joints.message())?;
    println!("{:?}", imu_sample);
    if let Some(sample) = joint_sample {
        for joint in sample.joints {
            println!("id={} position_rad={}", joint.id, joint.position_rad);
        }
    }
    Ok(())
}
```

### 固定窗口测量实际接收频率

运行只读示例 [`measure_observations`](../../examples/measure_observations.rs)，参数为 `<Gateway endpoint> [seconds=10] [both|imu|joints]`。显式指定 **Gateway gRPC 地址**，可选测量秒数接受1–3600的整数；第三个参数选择实际订阅的流，默认 `both` 保持原用法兼容，`imu`只订阅IMU，`joints`只订阅关节。指定模式时需同时填写秒数：

```powershell
cargo run --example measure_observations -- http://127.0.0.1:5052 10
cargo run --example measure_observations -- http://127.0.0.1:5052 60 imu
cargo run --example measure_observations -- http://127.0.0.1:5052 60 joints
cargo run --example measure_observations -- http://127.0.0.1:5052 60 both
cargo run --example measure_observations -- --help
```

示例使用真实 `DosoClient`，完成只读协议握手后只订阅选定的流，`both`模式并发订阅IMU和关节，不申请运动控制。所选订阅建立后开始同一墙钟窗口，只在结束或失败时汇总输出所选流的统计和实际模式：IMU消息数和Hz、含线加速度的消息数，或关节事件总数及每个关节0–15的条数和Hz、最大消费间隔、含源时间的事件数和源时间跨度。未选择的流不发起RPC，也不输出零值统计。Hz以条数除以实际观察窗口秒数计算；整帧关节事件中的每个关节各计一条。现有输出字段名 `max_receive_gap_ms` 保留，实际计时点是本例消费处理样本的时刻，包含窗口开始到首条、末条到窗口结束；它不是网络到达或硬件采样间隔。

源时间跨度只使用实际收到的 `source_elapsed`，为最后一个有时间的事件减去第一个，少于两个有时间事件时显示 `unavailable`，不会用电脑接收时钟补齐源时间。该跨度保留源时钟的含义；不能据此断言硬件采样同步或端到端延迟。

所选观察订阅2秒内未建立、消费者连续2秒未处理选定IMU或任一选定关节、窗口结束仍缺少所选数据、观察流提前结束或返回错误，示例均以非零状态退出。`imu`模式不检查关节，`joints`模式仍检查全部16轴但不检查IMU，`both`检查两者。运行期失败仍输出已消费的统计和 `completed=false`，不将部分窗口当作成功。结束或错误时丢弃已建立的流以取消订阅；本例不做重连，以免掩盖失败。可顺序运行三种模式对照单流与双流稳定性；某一模式成功不代表其他模式或长期稳定性通过。

消费超时会在stderr打印所有触发的具体 `imu` / `joint[id]`、`consumer_idle_ms`及最后一个已报告的源时间 `last_reported_source_ns`，随后只对两条已选流的读取future各轮询一次，不等待新数据。诊断为 `sample_ready(next_source_ns=...)`、`eof`、`rpc_error(code)` 或 `no_sample_ready`，未选流为 `not_selected`；诊断样本没有源时间时为 `unknown`。窗口尾部消费超时执行同样诊断。判失败后取得的诊断样本不计入原统计，仍立即以非零状态退出，不延长阈值或重排选择顺序使结果变成功。

该探针以消费时刻维护2秒计时，`biased`选择先检查窗口/空闲timer。当消费者经历调度停顿，过期timer与缓冲样本可能同时Ready，timer仍可能先触发；因此消费超时本身不足以证明Brainstem、网络或Gateway上游停流。`sample_ready`仅表示失败后的这一次轮询可以取到样本，连同其实际源时间供对照，不能确定样本何时从网络到达；`no_sample_ready`同样不等于上游已停止。定位需结合独立原始流和运行时证据。

`DOSO_API_KEY` 配置连接凭据。`https://` 使用 SDK 现有系统信任根；私有 CA 可通过本示例的 `DOSO_TLS_CA_FILE` 指向 PEM 文件，装入现有 `Config.tls_ca_certificate`，保留主机名校验：

```powershell
$env:DOSO_TLS_CA_FILE = 'C:\certs\robot-ca.pem'
cargo run --example measure_observations -- https://gateway.example:5052 10
```

私有 CA 仅适用于 HTTPS。Gateway 到 Brainstem 的传输与源装配由 Gateway 配置决定，不能将 Brainstem 主端口直接作为本示例的 SDK 地址。运行结果只证明所选 Gateway 的观察链路；真实硬件状态需要真实上游，不能用测试 fixture 替代现场测量。

## 电机诊断

### motors

```text
pub async fn motors(&self) -> Result<rpc::MotorStates>
```

无额外参数。调用 `Observations.Motors`，读取 Brainstem `GetMotorStatus`。当前适配器要求 16 个唯一 ID，覆盖 0–15。

每项返回 `id`、`online`、原始 `status_code`、温度 `temperature_c` 和原始故障码列表 `errors`。温度单位 °C。`online` 表示源服务近一秒观察到该电机通信。缺少反馈报告或电机离线时，源可能返回温度/状态 0 和空故障列表，报告有效性及采样时间仍缺少独立字段。设备健康应结合有效报告来源判断。

源未配置、上游失败、数量或 ID 集合不符合当前适配器合同，返回 `UNAVAILABLE` 或相应上游错误。

### motor_voltages

```text
pub async fn motor_voltages(&self) -> Result<rpc::MotorVoltagesResponse>
```

无额外参数。调用 `Observations.MotorVoltages`，读取 Brainstem `GetVoltage`。返回 `volts: Vec<f64>`，单位 V，当前要求 16 项并按关节索引排列。

源服务发起只读 CAN 电压查询，默认等待 500ms，未响应轴以 0 表示。响应没有逐轴有效位，调用方应保留 0 的这一来源语义。它提供电机母线电压，电池电量与 BMS 仍需独立来源。源未配置、上游失败或长度不为 16 时返回错误，频繁调用会增加 CAN 参数查询流量。

```rust,no_run
use doso_sdk::DosoClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    let observations = robot.observations();
    println!("{:?}", observations.scene().await?);
    let motors = observations.motors().await?;
    for motor in motors.motors {
        println!("id={} online={} temperature_c={}", motor.id, motor.online, motor.temperature_c);
    }
    println!("volts={:?}", observations.motor_voltages().await?.volts);
    Ok(())
}
```

## 点云订阅与本地解码

### watch_point_cloud

```text
pub async fn watch_point_cloud(
    &self,
    kind: rpc::CloudKind,
) -> Result<tonic::Streaming<rpc::CloudFrame>>
```

调用 `Observations.WatchPointCloud`，订阅现有 CloudViewer 地图或扫描显示帧。

| 参数 | 语义 |
| --- | --- |
| `CloudKind::Map` | 地图预览 |
| `CloudKind::Scan` | 扫描预览 |
| `CloudKind::Unspecified` | 本地输入错误 |

返回的 `CloudFrame.pcld` 为 PCLD 二进制帧，使用 `decode()` 得到可显示的点与元数据。首帧可能来自缓存，慢消费者可能跳过旧帧。默认显示规则约为地图最高 4Hz、扫描最高 10Hz，实际更新取决于源配置与处理。

源端进行显示采样和量化，调用方需要原始保存地图文件时使用 [maps 的 PCD 读取](maps.md)。点云源不可用、流配额耗尽和传输错误按公共约定返回。该客户端为点云设置独立消息解码容量，帧解码的有效 PCLD 大小上限为 10MiB。

### CloudFrame decode

```text
pub fn decode(&self) -> Result<PointCloud>
```

定义在 `doso_sdk::rpc::CloudFrame` 上，也可通过公共导出的 `doso_sdk::CloudFrame` 使用。无额外参数，读取本对象的 `pcld` 并执行本地解码。

返回 `PointCloud`。解码器要求 PCLD v2，检查帧长、版本、flags、点数、数据长度、坐标系 UTF-8，以及 scale、origin 和时间的有限性，最多 1,000,000 点。无效格式返回 SDK 协议错误。源码见[点云实现](../../src/observations/cloud.rs)。

### PointCloud 与 CloudStreamKind

`PointCloud` 是 SDK 解码结果，公开字段如下。

| 字段 | Rust 类型 | 语义 |
| --- | --- | --- |
| `points` | `Vec<[f32; 3]>` | 米制 XYZ |
| `colors` | `Option<Vec<[u8; 3]>>` | 可选 RGB，与点一一对应 |
| `frame_id` | `String` | 当前坐标系 |
| `epoch` | `u32` | 显示场景代次 |
| `sequence` | `u32` | 帧序号 |
| `stamp_s` | `f64` | 显示源时间，缺少源时间时可能回退服务端时间 |
| `stream_kind` | `CloudStreamKind` | `Cloud`、`Map`、`Scan`、`Reset` |
| `scale_m` | `f32` | 当前帧的量化步长，米 |

`CloudKind` 选择订阅来源，`CloudStreamKind` 描述收到的显示事件。`Reset` 应清空对应显示数据，坐标系或 `epoch` 变化时也应按新场景处理。

```rust,no_run
use doso_sdk::{CloudKind, CloudStreamKind, DosoClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    let mut frames = robot.observations().watch_point_cloud(CloudKind::Scan).await?;
    while let Some(frame) = frames.message().await? {
        let cloud = frame.decode()?;
        if cloud.stream_kind == CloudStreamKind::Reset {
            println!("reset epoch={}", cloud.epoch);
        } else {
            println!("frame={} points={} rgb={}", cloud.frame_id, cloud.points.len(), cloud.colors.is_some());
        }
    }
    Ok(())
}
```

## 当前来源边界

BMS、电池电量、足端和光照消息的定义见[消息清单](../message-catalog.md)，接收接口按真实数据源逐项开放。本文列出的接口需要目标设备部署相应服务，具体频率、时钟、关节映射及相机/点云表现仍需对应型号现场验收。

[返回 API 索引](README.md)
