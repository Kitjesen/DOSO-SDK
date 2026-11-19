# 相机图像和远程观看

## 当前视频链路

机器狗的图像链路使用 DOSO 对外的 **Video Gateway** 产品名。内部实现仍使用 `go2rtc` 作为视频会话和 H.264/WebRTC 转发组件。产品界面、SDK 状态和验收文档使用 `Video Gateway` 或 `DOSO Media Gateway`，部署配置和进程诊断保留 `go2rtc` 原名，避免改名后丢失上游配置和排障信息。

```text
Orbbec
  ↓
LingTu camera owner
  ↓
DOSO Video Gateway
  ├── HTTP :5050 /api/v1/camera/status
  ├── HTTP :5050 /api/v1/camera/snapshot
  └── WHEP :5050/api/v1/webrtc/whep?src=cam
       ↓
    App WebRTC receiver
```

Brainstem `13145` 继续只负责运动、IMU、关节和电机数据。视频不经过 Brainstem。

2026-11-18 的只读检查显示相机状态为 `streaming`，WHEP 状态列出 `cam`，JPEG 快照返回 `200 image/jpeg` 和 28,729 字节。[当前只读证据](D:/DOSO/artifacts/robot-cloud/real-connect-20261113/camera-current-readonly-20261118.json)

历史 WebRTC 解码验收显示连接成功、画面 `640×480`、首帧约 6.2 秒、解码帧持续增长、丢包为 0。[WHEP 验收证据](D:/DOSO/artifacts/robot-cloud/real-connect-20261113/real-webrtc-acceptance-20261115.json)

## SDK 与显示边界

App 已经可以通过 WHEP 接收并显示视频。Rust SDK 的 `DosoClient` 已有 JPEG 快照和 JPEG 流接口，前提是机器狗部署提供 `doso.v1` 的相机服务。`BrainstemClient` 不包含相机接口，因为 `brainstem.api.v1` 没有图像和视频 RPC。Rust SDK 负责连接、授权和接收数据，画面显示由 App 的 WebRTC 播放器或图像组件完成。

`observations().camera_frame(viewer_token)` 查询可取得的 JPEG 快照，`watch_camera(viewer_token)` 订阅 JPEG 帧。两者复用 LingTu 的相机快照和 JPEG relay。`CameraFrame.jpeg` 是完整 JPEG 字节，可交给图像解码器或写入 `.jpg` 文件。

```rust,no_run
use doso_sdk::DosoClient;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let robot = DosoClient::connect("http://机器人地址:5052").await?;
    let token = std::env::var("DOSO_MEDIA_VIEWER_TOKEN").ok();
    let frame = robot.observations().camera_frame(token.as_deref()).await?;
    let mut output = std::fs::File::create_new("camera.jpg")?;
    output.write_all(&frame.jpeg)?;

    let mut frames = robot.observations().watch_camera(token.as_deref()).await?;
    for _ in 0..10 {
        let Some(frame) = frames.message().await? else { break };
        println!("JPEG bytes: {}", frame.jpeg.len());
    }
    Ok(())
}
```

Gateway 每 100 ms 检查 JPEG 缓存序号，序号变化时交付一帧。实际接收率取决于相机、编码器及网络。订阅登记观看者后允许编码器冷启动，最多等待现有 5 秒帧停更阈值。相机明确未加载或报告错误时返回 `UNAVAILABLE`，已开始的流在 5 秒没有新帧时也以该码结束。取消或丢弃订阅会释放观看者登记和共享流名额。

快照沿用已有缓存读取语义；持续流等待订阅后的新发布序号再交付首帧。编码器可能重新编码已有原始图像，新发布和5秒停更检测都不能证明硬件新采样。`CameraSample` 的序号是服务发布计数；ID、尺寸和坐标系字段可缺省，采样时间仍无来源。图像单帧受客户端默认 4 MiB gRPC 消息解码上限约束。

仓库中的只读示例可将一帧保存到新文件，不覆盖已有路径：

```text
cargo run -p doso-sdk --example read_camera -- http://机器人地址:5052 camera.jpg
```

设备启用 `LINGTU_MEDIA_REQUIRE_TOKEN=1` 时，服务端从 `LINGTU_MEDIA_VIEWER_TOKEN` 读取查看凭据。调用方将同一凭据传入 `viewer_token`，SDK 使用 `authorization: Bearer ...` 元数据发送。缺少凭据返回 `UNAUTHENTICATED`，错误凭据返回 `PERMISSION_DENIED`，设备开启保护但没有配置凭据时返回 `UNAVAILABLE`。该规则与现有 HTTP 和 WebSocket 媒体入口共用判断逻辑。普通 `x-api-key` 仍按原有用途处理。

Gateway 默认 gRPC listener 为明文；查看令牌并不加密连接。通过可信网络或已验证的 TLS 终止层访问，勿在不可信网络中直接发送明文凭据。

## Web 观看入口

设备部署并运行 Gateway、相机及视频服务后，可以打开以下地址。

```text
http://机器人地址:5050/?tab=scene
http://机器人地址:5050/api/v1/camera/snapshot
http://机器人地址:5050/api/v1/camera/status
http://机器人地址:5050/api/v1/webrtc/status
```

Scene 页面使用现有 WebRTC/H.264 路径并支持 JPEG 预览。局域网或可路由 VPN 需要同时保证信令与媒体 ICE 地址可达。跨公网播放需要相应网络部署和设备验证。

本轮图像接口通过本地测试验证。当前 Thunder 现场视频服务和持续新帧输出尚待只读验收，设备安装版本需要支持新增 RPC。
