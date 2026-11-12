# DOSO Rust V1 推进记录

> 历史记录：以下内容保留当时的实现、路径与验证结论。当前主线见[使用说明](../usage.md)和[gRPC 推进记录](../grpc-v1-progress.md)。 当时 `build/` 下的本地验证证据现保存在 `.local/evidence/`。

日期：2026-11-02。主语言：Rust。预览版本：`doso-sdk 0.1.0-alpha.1`。当前达到 M1 本地开发预览，尚未正式发布或部署。

## 环境与交付

- 本机已有 Rust/Cargo 1.94.0、Windows x86_64 MSVC、rustfmt 和 Clippy；无需重复安装工具链。
- 根 `Cargo.toml` 管理 `rust/doso-sdk`，Cargo.lock 记录已验证的依赖解析。HTTP/TLS 使用 reqwest 0.12.28；不需要 Python 运行时或完整 LingTu 安装包。
- 七域均有 Rust 入口，首轮范围对齐已有 Python 预览：状态、软件急停、定位与重定位、地图及 ProductControl 事务、导航目标与任务查询/取消/等待、场景/相机状态、Agent instruction/run。
- 使用 `Pose`、`CommandReceipt`、`Error/ErrorKind` 表达输入与结果，保留服务端 JSON 证据和请求身份。
- 禁止客户端自动请求重试和 HTTP 重定向；保留 TLS 验证；不使用桌面系统代理。写请求断线、非法回执、408/5xx 保留未知提交结果，不伪装为确定未执行。
- 库通过 Cargo 依赖使用，不通过 `cargo install` 安装。只读示例是 `read_status`；README 已给出本地依赖和使用方式。

## 本地验证

| 检查 | 结果 | 证据范围 |
| --- | --- | --- |
| Cargo 编译 | 通过 | Windows / Rust 1.94 / MSVC |
| rustfmt | 通过 | workspace 格式 |
| Clippy `-D warnings` | 通过 | workspace 所有 targets |
| 单元测试 | 5 passed | 任务/地图终态、身份、矛盾证据、等待预算 |
| HTTP 合同测试 | 16 passed | std TCP 本地假服务；路径/请求、七域、未知写结果、禁止重试和重定向等 |
| 文档示例编译 | 2 passed | README Rust 片段是 no_run 编译检查，不执行运动 |
| Cargo package | 通过，且解包后验证编译 | `target/package/doso-sdk-0.1.0-alpha.1.crate` |
| 独立应用接入 | 通过 | 独立 Cargo consumer 从打包解开的 crate 建立 path 依赖，离线构建并运行本地合同探针 |
| 真实 Gateway 请求模型 | 8 次请求通过 | 上述 Rust consumer 产生的 HTTP JSON，经当前 LingTu 实际 Pydantic 请求模型校验；并未运行真实机器人执行服务 |
| 只读示例帮助 | 通过 | `cargo run -p doso-sdk --example read_status --locked -- --help`，没有联网 |
| 远端 CI / Linux | 未运行 | 已有 Windows/Ubuntu CI 配置，配置存在不算通过 |

独立消费者检查脚本和请求证据保存在本地构建目录 `build/verify_rust_consumer.py`、`build/rust-gateway-schema-evidence.json`，它们是开发验证产物，不是 SDK 运行依赖。

初次 Windows HTTP 测试曾因 accepted socket 继承非阻塞模式而出现 WouldBlock，测试清理期间二次 panic 导致进程中止。已在测试服务器 accept 后显式恢复 blocking，并避免 panic 清理时再次 panic；随后默认并发的完整测试通过。没有为此放宽生产客户端的超时、错误或重试语义。

## 源码目录整理

按用户要求，Rust 七个领域已改为七个目录，每项公开功能独立一个同名 `.rs` 文件，共 31 个功能文件。领域 `mod.rs` 仅保留模块声明与入口结构体；地图域既有的共享内部逻辑位于 `switch.rs` 和 `deadline.rs`。

公开方法的签名与实现逻辑保持；原有 5 个单元测试随所属功能迁移，16 个 HTTP 合同测试和 2 段文档编译检查全部通过。rustfmt、Clippy、Cargo package 及解包编译通过；文档链接和文件归属检查通过。此项是源码组织调整，gRPC 迁移仍见 [通信方案](../transport-selection.md)，尚未实施。

随后将共享代码也归入 `client/`、`common/`、`error/`、`models/`，`src` 根层只保留 `lib.rs`。公开导出和方法实现保持；完整格式、Clippy、上述 23 项测试/文档检查和打包验证再次通过。

## 与历史版本的关系

Python `0.1.0a1` 保留为已做工作的合同参考，历史记录仍在 [v1-progress.md](v1-progress.md)，不再作为 V1 主实现扩展；它的 56 项测试不能算作 Rust 证据。现有 Brainstem C++ 客户端、未提交 profile 改动和机器人运行代码保留。

## 下一阶段

按照 [V1 计划](../v1-plan.md) 进入 M2：先补导航暂停/恢复的实际 owner 接线，再确定身体动作和连续遥操作的整机入口与交接合同。IMU/关节/点云/图像帧订阅、巡检/跟随的统一封装也尚未完成。

主 Gateway 当前没有 API Key 认证中间件，发送 header 不能代替鉴权；生产访问保护需单独实现与验收。未运行仿真运动或实机，没有推送、发布 crate 或部署机器人。Rust 本身不是硬实时、规划性能或运动安全证明。
