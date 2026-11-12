# 连接与公共接口

`DosoClient` 管理一个共享gRPC通道、最近成功握手的信息及每次请求的元数据。七个领域通过同一个客户端访问。机器人端服务默认使用5052端口。[客户端源码](../../src/client/mod.rs)

## Config

```text
pub fn Config::new(endpoint: impl Into<String>) -> Config
```

| 字段 | Rust类型 | 默认值与含义 |
| --- | --- | --- |
| `endpoint` | `String` | 构造时传入，格式为`http://主机:端口`或`https://主机:端口` |
| `client_id` | `String` | 默认`doso-sdk`，随握手和写请求提供调用方身份，必须非空 |
| `api_key` | `Option<String>` | 默认`None`，设置后随RPC发送`x-api-key`元数据 |
| `timeout` | `Duration` | 默认10秒，必须大于0，用于连接和普通请求等待 |
| `tls_ca_certificate` | `Option<Vec<u8>>` | 默认`None`，为HTTPS追加PEM私有CA，保留系统信任根和主机名校验 |

地址中的`http://`选择gRPC明文HTTP/2通道，`https://`选择TLS。证书必须匹配地址中的主机名。设置CA要求使用HTTPS且PEM字节非空，非法元数据或参数返回`InvalidArgument`。TLS信任或握手失败通过连接错误返回。

```rust,no_run
use doso_sdk::{Config, DosoClient};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = Config::new("https://robot.example:5052");
    config.client_id = "inspection-tool".into();
    config.api_key = Some(std::env::var("DOSO_API_KEY")?);
    config.tls_ca_certificate = Some(std::fs::read("robot-ca.pem")?);
    config.timeout = Duration::from_secs(10);
    let robot = DosoClient::with_config(config).await?;
    println!("{:?}", robot.system().info());
    Ok(())
}
```

机器人端的API key和TLS由服务配置提供，具体环境变量集中在接入指南中维护。[服务端凭据配置](../usage.md#api-key-与-tls)

## connect

```text
pub async fn DosoClient::connect(endpoint: impl Into<String>) -> Result<DosoClient>
```

| 参数 | 含义 |
| --- | --- |
| `endpoint` | 机器人gRPC服务地址，由应用提供 |

使用`Config::new(endpoint)`的默认值，先建立通道，再调用`System.Handshake`，核对协议版本并取得`SystemInfo`，成功后返回`DosoClient`。

```rust,no_run
use doso_sdk::{DosoClient, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let robot = DosoClient::connect("http://127.0.0.1:5052").await?;
    println!("{:?}", robot.system().info());
    Ok(())
}
```

连接采用观察式握手。当前调用需要已知endpoint；连接本身不申请控制权，控制权通过 `motion().claim_control()` 显式申请。握手信息包含机器人ID、机器人版本和固件版本，详细语义见system页面。[system](system.md) · [控制权边界](motion.md)

## with_config

```text
pub async fn DosoClient::with_config(config: Config) -> Result<DosoClient>
```

采用指定配置建立连接，执行与`connect()`相同的应用握手。配置对象被该调用取得，连接后七个领域共享通道、凭据和等待规则。

| 常见结果 | 调用行为 |
| --- | --- |
| `Unavailable` | 地址不可达、TLS或通道建立失败，或服务依赖不可用 |
| `Unauthenticated` | 已连接到启用鉴权的服务，凭据未通过 |
| `Unimplemented` | 目标服务尚未提供`System.Handshake` |
| `FailedPrecondition` | 服务拒绝协议版本或配置身份，`reason_code`可提供`protocol_mismatch/invalid_robot_id` |
| `Internal`，本地`ErrorKind::Protocol` | 收到不匹配的握手版本或缺少SystemInfo |

所有错误码和原始详情通过统一`Error`读取。[返回码](../results.md)

## reconnect

```text
pub async fn DosoClient::reconnect(&mut self) -> Result<()>
```

沿用客户端原有地址、TLS、凭据、调用方身份和超时配置，建立新的gRPC通道并重新执行只读握手。成功后替换客户端通道，`system().info()` 返回这次握手的身份快照。连接等待和握手请求分别受 `Config.timeout` 约束。

已有 `robot_id` 时，新握手必须返回相同ID；ID改变或消失返回 `ErrorKind::Protocol`。初次握手没有ID时，允许后续握手获得ID。该检查适用于本方法显式重连时的握手，不代表每次普通RPC都重新验证机器人身份。

连接、鉴权、协议或身份检查失败时返回错误，保留旧通道与上次成功的身份快照；旧快照不代表本次连接成功，也不保证旧通道当前可用。需要改地址或凭据、连接另一台机器人时，使用 `connect/with_config` 创建新的客户端。

```rust,no_run
use doso_sdk::DosoClient;

async fn reconnect(robot: &mut DosoClient) -> Result<(), Box<dyn std::error::Error>> {
    robot.reconnect().await?;
    println!("{:?}", robot.system().info());
    let mut imu = robot.observations().watch_imu().await?;
    println!("{:?}", imu.message().await?);
    Ok(())
}
```

重新连接不恢复已有订阅、不重放控制命令、不重新申请控制权。普通 `tonic::Streaming` 仍属于原通道，需要调用方结束旧流并重新订阅；借用客户端的 `Teleop/ExternalControl` 会话需要先结束，才能取得 `&mut DosoClient`。重新申请控制权前应读取当前状态，继续遵守机端仲裁。

## 领域访问

```text
pub fn DosoClient::system(&self) -> System<'_>
pub fn DosoClient::motion(&self) -> Motion<'_>
pub fn DosoClient::localization(&self) -> Localization<'_>
pub fn DosoClient::maps(&self) -> Maps<'_>
pub fn DosoClient::navigation(&self) -> Navigation<'_>
pub fn DosoClient::observations(&self) -> Observations<'_>
pub fn DosoClient::tasks(&self) -> Tasks<'_>
```

这些同步方法返回借用当前客户端的领域句柄，访问方法本身不执行网络请求。领域的查询、命令或订阅方法分别执行对应RPC。

## new_request_id

```text
pub fn new_request_id() -> Result<String>
```

生成规范ULID，用于在提交写请求前保留请求身份。发生系统时间或随机源错误时返回`Error`。

```rust,no_run
use doso_sdk::{new_request_id, DosoClient, Result};

async fn request_stand(robot: &DosoClient) -> Result<()> {
    let request_id = new_request_id()?;
    match robot.motion().stand_up(Some(&request_id)).await {
        Ok(receipt) => println!("{} {:?}", receipt.request_id, receipt.accepted),
        Err(error) => {
            eprintln!("{} {:?} {:?}", request_id, error.code(), error.reason_code());
            if error.submission_unknown {
                eprintln!("request outcome requires checking robot state");
            }
            return Err(error);
        }
    }
    Ok(())
}
```

`CommandReceipt.request_id`与原命令身份不匹配时返回协议错误，保留原请求ID及未知提交事实。该错误的查询处理与普通超时共用公共约定。[请求与结果约定](README.md#请求身份)

## 资源结束与重新连接

Rust对象管理客户端和流资源。普通订阅通过丢弃其`tonic::Streaming`结束，遥操作需要确认停止与释放时调用`Teleop::close()`。保留订阅流时，它继续持有自身的连接资源。

Tonic可为普通RPC重建底层传输连接，但不会替应用重新握手、刷新身份快照或恢复业务流。`reconnect()` 显式建立新通道并核对握手；再次调用 `connect/with_config` 可建立独立客户端。应用可通过 `ping()` 和所需领域的查询观察当前服务。当前不提供独立连接状态事件或自动恢复业务订阅。[system.ping](system.md) · [能力缺口](../v1-plan.md#对标后的能力缺口)

## Pose::validate

```text
pub fn Pose::validate(&self) -> Result<()>
```

检查公共位姿`x/y/z/yaw`均为有限值。位置单位m，yaw单位rad，非有限值返回`InvalidArgument`。导航与带种子的重定位在提交前使用同一检查，所需坐标系由对应领域说明。[导航](navigation.md) · [重定位](localization.md)
