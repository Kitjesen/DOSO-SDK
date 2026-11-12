# SDK 通信方案

日期：2026-11-02。七域 Rust SDK 已采用 **gRPC + Protobuf**，客户端使用 **Tonic 0.14 + Prost 0.14 + Tokio**。当前实现见 [Rust SDK](usage.md)，验证记录见 [gRPC 推进记录](grpc-v1-progress.md)。

## 实施决定

```text
应用 → DOSO Rust SDK → gRPC / Protobuf :5052
                       → LingTu gateway.rpc → 共享领域操作 → 现有 owner
```

协议统一维护于 [`proto/`](../proto/)。Rust 构建生成客户端与消息；`python scripts/generate_grpc.py --lingtu ../lingtu` 生成机器人端 Python bindings。七域共享连接，核心调用全部采用 async，导航任务状态采用 server streaming。

`http://127.0.0.1:5052` 是明文 HTTP/2 gRPC endpoint，消息使用 Protobuf。gRPC 基于 HTTP/2，使用这个地址前缀不代表继续发送 REST/JSON。当前没有通信基准结果，不承诺固定的延迟或吞吐提升比例。

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
