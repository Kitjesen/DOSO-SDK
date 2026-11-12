# 机器人信息与固件发布

文档核对日期为2026-11-08。SDK 的 `system().info()` 统一提供三项信息，当前云端设备开通和下一阶段见 [robot-cloud推进记录](D:/DOSO/robot-cloud/PROGRESS.md)。核对日期不表示已发布新固件或完成设备登记验收。

| 信息 | SDK 字段 | 含义与来源 |
| --- | --- | --- |
| 固件版本 | `firmware_version` | 统一机器人软件发布号，读取当前运行发布包 `metadata.json` 的 `version` |
| 机器人版本 | `robot_version` | 型号与硬件代际，沿用 `vendor/model`，读取 RunPlan 或 `LINGTU_ROBOT` |
| 机器人 ID | `robot_id` | 这台机器的永久 UUID，读取设备环境变量 `DOSO_DEVICE_ID` |

三项均允许缺失，Rust 中表示为 `None`。开发源码运行且没有发布元数据时，固件版本保持缺失。连接取得的是初次握手快照，动态运行环境、会话、健康与就绪分别通过现有状态接口查询。字段定义见 [SystemInfo](../proto/messages/system.proto)。

## 永久机器人 ID

新设备选择 **UUID v4**，格式为小写、带横线的 36 字符字符串。云端设备库存登记时，复用 `devices.id` 的 `gen_random_uuid()` 默认生成；已有设备沿用已登记 ID。SDK 和 App 不生成另一份机器人 ID。

设备端复用现有持久配置 `/etc/doso/robot-cloud/agent/agent.env` 的 `DOSO_DEVICE_ID`。示例仅说明格式，不代表已登记设备：

```text
DOSO_DEVICE_ID=8f7e1c62-0a4d-4b89-96e2-6c47b03db1a5
```

统一关系为 `SDK.robot_id = doso-cloud-link.DOSO_DEVICE_ID = 云端 devices.id`。设备证书与MQTT主题继续使用同一ID。登记后保持不变，重启、固件升级和用户解绑不重新分配；App绑定只改变设备的账号归属。云连接机器狗端源码现位于 `D:\DOSO\robot-cloud\robot/`，安装配置继续使用上面的 `agent/agent.env` 路径。

LingTu Host与 `doso-cloud-link` 的systemd配置共用同一个 `agent.env`，SDK握手只读取其中的 `DOSO_DEVICE_ID`；配置接线不代表机器狗已安装并启动该服务。文件位于发布目录之外，升级包不重新分配ID，登记值在进程启动时加载。直接启动开发进程时设置同名环境变量即可。

变量未设置或为空时，SDK 返回 `robot_id=None`；非空值必须是规范 UUID v4，否则连接返回 `FAILED_PRECONDITION`，原因是 `invalid_robot_id`。SDK 不回退到型号配置的 `thunder_v4_001` / `go2_001`，也不自行生成 ID。此处校验格式，设备是否已登记仍由云端确认。

## 一次发布，一个固件版本

复用现有打包与安装流程，由发布包的 `metadata.json.version` 统一表示交付的软件版本。组件依赖、构建提交和协议兼容信息由现有构建与发布产物记录，使用者管理上述三项即可。发布入口见 [LingTu 发布说明](../../lingtu/scripts/deploy/README.md)。

当前发布包覆盖 LingTu 原生运行时，Brainstem 固件纳入整机统一交付仍待接入。首台设备的独立 ID、实际固件版本与安装验收结果也仍待实机确认。

2026-11-07另有已登记模拟库存、正常API绑定和实际Web App状态流程的验证，库存显式标为模拟，详见 [SIMULATED_FLOW.md](D:/DOSO/robot-cloud/docs/SIMULATED_FLOW.md)。该记录不证明首台真实机器狗身份已开通、三项信息已可信上云，或整机固件发布已完成。

智身握手同样提供机型与系统版本信息，DOSO 按本项目要求将对外信息收敛为以上三项。参考固定提交中的[握手响应 §5.2](https://github.com/zsibot/genisom_robot_sdk/blob/e8f959a22a6e5714bc299a551b45e1c182ac05c7/docs/zh/protocol/Protocol-1.3.0.md)。
