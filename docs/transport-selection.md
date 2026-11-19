# SDK 通信方案

日期：2026-11-02。七域 Rust SDK 已采用 **gRPC + Protobuf**，客户端使用 **Tonic 0.14 + Prost 0.14 + Tokio**。当前实现见 [Rust SDK](usage.md)，验证记录见 [gRPC 推进记录](grpc-v1-progress.md)。

## 当前实施决定

本地控制路径直接使用 Brainstem 原生 gRPC，不部署额外的 5052 Gateway：

```text
应用 → DOSO Rust SDK BrainstemClient → gRPC / Protobuf :13145 → Brainstem
```

`13145` 只由 Brainstem 持有，负责 CAN、IMU、关节、电机和运动仲裁。SDK 不打开硬件设备，也不创建第二个硬件 owner。云连接使用回环只读端口 `13146`，不能发送运动命令。

七域高层合同仍保留为可选 LingTu 接入：

```text
应用 → DOSO Rust SDK → gRPC / Protobuf :5052
                       → LingTu gateway.rpc → 共享领域操作 → 现有 owner
```

协议统一维护于 [`proto/`](../proto/)。Rust 构建生成客户端与消息；`python scripts/generate_grpc.py --lingtu ../lingtu` 生成机器人端 Python bindings。七域共享连接，核心调用全部采用 async，导航任务状态采用 server streaming。

`http://127.0.0.1:13145` 是当前 Rust `BrainstemClient` 使用的明文开发 HTTP/2 gRPC endpoint，消息使用 Protobuf。该客户端当前不接受 `https://`，所以 `13145` 只能在受控的本地网络使用，生产 mTLS 接入需要先补齐客户端 TLS 配置。`5052` 只在未来部署 LingTu Gateway 时启用，当前产品路径不依赖它。

机器人端 handler 调用现有共享操作和 owner，没有通过旧 Gateway REST 代理七域业务。Web 的 HTTP :5050 继续调用相同操作；ProductControl 仍使用本机 HTTP :5051。原生 DDS、mapd socket 和 Brainstem gRPC 保留各自职责。

## 选择依据

| 方案 | 官方能力与本项目用途 |
| --- | --- |
| [Tonic / gRPC Rust](https://github.com/grpc/grpc-rust) | HTTP/2、异步 Rust、代码生成与流式 RPC；作为七域远程服务合同 |
| [Prost](https://github.com/tokio-rs/prost) | Protobuf 消息生成；双方使用同一份字段定义 |
| [Zenoh](https://github.com/eclipse-zenoh/zenoh) | 发布订阅、query/reply 与可选共享内存；当前没有需要增加这层部署的实测瓶颈 |
| [CycloneDDS](https://github.com/eclipse-cyclonedds/cyclonedds) | 已有机内数据分发；继续支撑原生进程通信 |

RPC 和流式模型见 [gRPC 官方概念说明](https://grpc.io/docs/what-is-grpc/core-concepts/)。当前选择已发布的 Tonic 0.14 系列，依赖版本由 Cargo 锁定。

## 合同与下一步

命令回执、位姿、任务、地图操作和产品操作已有明确的 Protobuf 字段。诊断状态保留 `google.protobuf.Struct`，便于承接现有字段；后续根据开发者实际使用需要逐步稳定诊断合同。

协议迁移不补齐缺失的机器人能力。导航暂停/恢复和主要观测流已有接线；站起/坐下、连续遥操作与控制权交接仍需推进，再按真实环境验证。当前 listener 使用明文 gRPC，整机 API key 鉴权尚未实施；服务部署与现场验收独立推进。
