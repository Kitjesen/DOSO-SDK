# 直接 Brainstem 接入

当前机器狗本地 SDK 直接连接 Brainstem 的主 gRPC 端口 `13145`。这条路径不需要部署独立的 `5052` Gateway。

```text
App / 本地 Rust 程序
        ↓ BrainstemClient
机器狗 Brainstem :13145
        ↓
CAN、IMU、关节、电机与运动仲裁

doso-cloud-link → Brainstem :13146 → GetControlStatus（只读）
```

## 当前可用能力

`BrainstemClient` 暴露 Brainstem 原生 `brainstem.api.v1` 合同，包含：

- 控制状态、Profile、电机状态和电机母线电压
- IMU 与关节持续流
- 控制权租约、站起、坐下、50Hz 速度命令
- 电机使能/禁用、Profile 和速度模式切换
- 清除电机故障、电机清零

```rust,no_run
use doso_sdk::{brainstem::rpc as brainstem_rpc, BrainstemClient};

async fn motion(robot: &BrainstemClient) -> doso_sdk::Result<()> {
    let lease = robot.acquire_control("my-controller").await?;
    let token = lease.token;
    robot
        .walk_checked(
            token.clone(),
            1,
            brainstem_rpc::Vector3 { x: 0.0, y: 0.0, z: 0.0 },
        )
        .await?;
    robot.release_control(token).await?;
    Ok(())
}
```

`release_control` 只释放租约，不代替停止命令。持续遥操作应由调用方按 20ms 周期提交递增序号，并在结束前发送零速度、检查回执，再释放租约。

### External 逐关节控制

`robot.external()` 返回同一 `13145` 连接上的 `BrainstemExternalClient`。它提供 `enter`、`enable`、`control` 双向流、`heartbeat`、`disable`、`exit` 和 `status`。逐帧目标必须包含 16 个关节、递增 `sequence` 和 Brainstem 签发的 `input_window`。SDK 不自动重发或在断线后重放关节目标；断线时由 Brainstem 的输出期限和安全门接管。

客户端只调用 Brainstem 的 gRPC 服务，不打开 CAN、不访问串口，也不维护第二份电机状态。控制命令仍受 Brainstem 的租约、状态机和安全输出限制。

## 未纳入直接路径的能力

Brainstem 协议没有地图、SLAM 定位、导航任务、地图文件或高层任务服务。七域 `DosoClient` 和 `doso.v1` 协议保留这些能力的统一合同，后续连接 LingTu 对应 owner 时再启用，不在当前直接入口中伪造实现。

## 只读示例

```powershell
cargo run --example read_brainstem -- http://192.168.66.64:13145
```

示例只读取状态、Profile、电压和电机状态，不申请控制权、不使能电机、不发送运动命令。控制示例中的零速度仅说明调用顺序，不代表已经在真机执行。

## 端口边界

| 端口 | 用途 | 外部 SDK 是否使用 |
| --- | --- | --- |
| `13145` | Brainstem 主 gRPC，含观测和本地运动控制 | 是，使用 `BrainstemClient` |
| `13146` | Brainstem 回环只读状态入口 | 否，仅供机端云连接 |
| `5052` | LingTu 七域 Gateway 合同 | 当前不要求部署 |

云端不获得 `13145` 的通用控制隧道。云端状态仍从机端只读桥读取，运动控制保留在本地 SDK。
