# BrainstemClient

`BrainstemClient` 直接连接机器狗 `brainstem.api.v1` 服务，默认地址是 `http://127.0.0.1:13145`。它不经过 `5052`，也不打开 CAN 或串口。

它使用 Rust 编写，基于 Tokio 异步运行时、Tonic gRPC 和 Prost Protobuf。公共导出位于 `src/lib.rs`，实现位于 `src/brainstem/mod.rs`。

`http://` 适用于明文服务的机上回环与现有局域网调试。启用 mTLS 的设备使用 `https://`，配置设备 CA 和已配对的客户端证书。证书须匹配连接地址。

```rust
use doso_sdk::{BrainstemClient, BRAINSTEM_DEFAULT_ENDPOINT};

let robot = BrainstemClient::connect(BRAINSTEM_DEFAULT_ENDPOINT).await?;
let status = robot.control_status().await?;
let motors = robot.motor_status().await?;
```

```rust
use doso_sdk::{brainstem::{Certificate, ClientTlsConfig, Identity}, BrainstemClient, BrainstemConfig};

let tls = ClientTlsConfig::new()
    .ca_certificate(Certificate::from_pem(std::fs::read("ca.cert.pem")?))
    .identity(Identity::from_pem(std::fs::read("client.cert.pem")?, std::fs::read("client.key.pem")?));
let robot = BrainstemClient::with_config(BrainstemConfig::new("https://thunder-01.local:13145").with_tls(tls)).await?;
let status = robot.control_status().await?;
```

`read_brainstem` 和 `measure_brainstem` 在 HTTPS 模式下从 `DOSO_BRAINSTEM_CERT_DIR` 读取这三个 PEM 文件。订阅建立超时使用 `BrainstemConfig.timeout`，已建立的观测流可持续读取。

## 查询与流

| 方法 | 返回 |
| --- | --- |
| `control_status()` | 身体状态、输出门、控制权和租约状态 |
| `cms_state()` | 当前 `CmsState`（状态、过渡动作和动作名称） |
| `profile()` | 当前 Profile、可用列表及 `2.3` 模式能力（`mode_capabilities` / `active_mode_id`） |
| `list_gestures()` | 机端可用预设动作列表及预计时长 |
| `voltage()` | 各电机母线电压 |
| `motor_status()` | 电机在线、温度、位置、速度、力矩和故障 |
| `speed_mode()` | 当前速度模式（未实现：机端当前返回 `UNIMPLEMENTED`） |
| `listen_cms_state()` | `CmsState` 状态变更服务端流 |
| `listen_imu()` | IMU 服务端流 |
| `listen_joint()` | 关节服务端流 |

## 运动与控制权

| 方法 | 说明 |
| --- | --- |
| `acquire_control(client_id)` | 申请 Brainstem 控制租约 |
| `renew_control_lease(token)` | 续租 |
| `release_control(token)` | 释放租约 |
| `enable_motors()` / `disable_motors()` | 电机输出门操作 |
| `stand_up()` / `sit_down()` | 姿态动作 |
| `damp()` | 触发解析式阻尼软急停（`Damp`），100ms 斜坡切入纯阻尼并在静止后收敛至 `Grounded` |
| `play_gesture(name)` | 播放机端预设动作（非空名称校验；需站立态） |
| `walk_checked(token, sequence, direction)` | 带租约和序号的机身速度命令 |
| `switch_profile(name)` | 切换内置 Profile |
| `set_speed_mode(mode)` | 设置速度模式（未实现：机端当前返回 `UNIMPLEMENTED`） |
| `clear_motor_fault(joint_ids)` | 清除指定或全部电机故障 |
| `set_zero()` | 在 Brainstem 允许时保存当前位置零点 |

`walk_checked` 的 `direction` 使用机体系单位，`x/y` 为 m/s，`z` 为 rad/s。调用方应持续发送序号递增的输入，并在停止或断连时释放租约。SDK 不会自动重试运动命令，也不会在重连后重放命令。

## External 策略

`robot.external()` 返回同一 Brainstem 连接上的 `BrainstemExternalClient`。它提供 `enter`、`enable`、`heartbeat`、`control`、`disable`、`exit` 和 `status`，用于客户自己的逐关节策略。进入 External 前必须由 Brainstem 接纳会话，帧流断开后 SDK 不重放旧帧。

> [!NOTE]
> **v0.0.1 服务状态**：`RobotExternalControl` 已接入 Brainstem `v0.0.1` 双层策略内核（[HardwareExternal](../../../brainstem/thunder/lib/src/external/hardware_external.dart) + [RealJoint SafetyGate](../../../brainstem/thunder/lib/src/real_joint.dart)，待部署至 `THUNDER-DEV-001` 并完成真机安全窗口验收）；`RobotRecovery` 仍通过 `brainstem.env` 关闭（`THUNDER_RECOVERY_MODEL_CONFIG` 未设置），在未验收前不挂载。

## 只读稳定性测试

使用 `measure_brainstem` 做固定窗口验收：

```powershell
cargo run --release --example measure_brainstem -- https://thunder-01.local:13145 900 10 [out_dir]
```

默认窗口为 900 秒。程序订阅 IMU 和关节流，每 10 秒查询控制状态、Profile、电压和电机状态。IMU 和 16 轴分别统计接收空窗与机端单调时间戳间隔，并校验 IMU 角速度/四元数有限性与 `linear_acceleration`（m/s²）。频率要求 98–102 Hz、空窗最多 2 秒，查询失败、流提前结束、缺轴或数据异常（含缺失/非有限加速度）均返回非零退出码。

指定第 4 个可选参数 `out_dir` 时，会在开始时写入 `meta.json`（含起止参数），每 60 秒向 `checkpoints.jsonl` 追加并落盘（`sync_data`）一行分段快照，结束时原子写入 `summary.json`，避免长稳测试期间开发机意外重启丢失全部进度。

需要单独测量传感器流时，将查询间隔设为 `0`：

```powershell
cargo run --release --example measure_brainstem -- https://thunder-01.local:13145 900 0
```

该模式不执行电压和状态查询，只用于区分实时流本身与查询并发造成的影响。

## 原生协议中暂未封装的方法

`brainstem.api.v1` 还定义了仿真步进、历史帧、模型参数等方法，以及 `RobotRecovery` 服务。当前 Rust 客户端暂未为以下 RPC 提供便捷方法：`Walk`、`Tick`、`Step`、`GetStartTime`、`ListenHistory`、`GetParams` 以及 `RobotRecovery`。生成的原生客户端仍可通过 `brainstem::rpc` 访问这些类型，正式封装前需要先确认真机服务实现和安全语义。
