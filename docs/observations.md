# 机器人观测能力

SDK 对外提供查询和订阅，数据由现有机器人端服务产生。下表描述当前代码接线，不代表已在每台机器上部署或验收。具体方法及运行方式见 [使用说明](usage.md)，验证记录见 [gRPC 推进记录](grpc-v1-progress.md)。

| 数据 | 当前 SDK | 机器人端来源 | 后续工作 |
| --- | --- | --- | --- |
| IMU | `BrainstemClient::listen_imu()` 流 | Brainstem `ListenImu` | 目标 100 Hz，连续交付情况见[真机验收第 3 项](REAL_ROBOT_ACCEPTANCE_MATRIX.md) |
| 关节位置、速度、力矩 | `BrainstemClient::listen_joint()` 流 | Brainstem `ListenJoint` | 16 轴分别验收频率和接收间隔，见[真机验收第 3 项](REAL_ROBOT_ACCEPTANCE_MATRIX.md) |
| 电机在线、温度、状态及故障码 | `observations().motors()` 单次查询 | Brainstem `GetMotorStatus` | 源增加报告有效性信息；离线默认值不能证明健康 |
| 电机母线电压 | `BrainstemClient::voltage()` 单次查询 | Brainstem `GetVoltage` | 已验证与 IMU、关节流并行的 15 分钟周期查询，见[测试记录](../TESTING.md) |
| 系统、就绪与健康 | `system()` 多项查询 | Gateway、ProductControl 等 | 按真实产品完善字段和设备验收 |
| 身体状态 | `motion().body_state()` 查询、`watch_body_state()` 事件流 | Brainstem 身体状态机 | 实机验证状态转换 |
| 地图位姿和定位状态 | `localization().pose()`、`status()` 单次查询 | LingTu 定位与 SLAM | 位姿订阅及设备验收 |
| 建图和地图 | `maps()` 查询及操作 | ProductControl、mapd、SLAM | 地图数据交付及设备验收 |
| 导航状态和任务 | `navigation()` 查询、任务操作及状态流 | LingTu navd | 暂停、恢复及设备验收 |
| 相机状态及 JPEG 图像 | `observations().camera_status()`、`camera_frame()`、`watch_camera()` | Gateway 相机健康与 JPEG relay | 相机内参及实机图传验收 |
| 保存地图的点云 | `maps().point_cloud()` 分块读取 PCD | mapd 保存的地图文件 | 实机文件与传输验收 |
| 实时点云 | `observations().watch_point_cloud()` 地图或扫描显示帧 | LingTu CloudViewerService | 实机频率、时延与 App 显示验收 |
| 电池电量、充电状态 | 暂无 SDK 接口 | 尚未找到可确认的实机 BMS 数据源 | 确认设备数据来源后接入 |

电机诊断没有提供源数据采样时间或报告有效位。电压查询通过 CAN 按需读取各电机母线电压。电池电量和充电状态需由独立数据源提供。

## 对照官方 SDK

智身 [GEN2 类型](https://github.com/zsibot/genisom_robot_sdk/blob/e8f959a22a6e5714bc299a551b45e1c182ac05c7/docs/zh/sdk_type_zh.md#L213-L385) 和 [回调](https://github.com/zsibot/genisom_robot_sdk/blob/e8f959a22a6e5714bc299a551b45e1c182ac05c7/docs/zh/sdk_callback_zh.md#L57-L118) 分别提供综合状态、IMU、关节、运控数据和故障事件。身体姿态与机器人业务状态是不同概念；DOSO 同样从各自 owner 获取，不从单一传感器推断。

智身同一固定版本资料对 IMU 频率有差异：回调文档称可配置 0–100 Hz，[协议文档](https://github.com/zsibot/genisom_robot_sdk/blob/e8f959a22a6e5714bc299a551b45e1c182ac05c7/docs/zh/protocol/Protocol-1.3.0.md#L609-L627) 则说固定上限且忽略该频率参数。DOSO 按自己的数据源能力说明频率，不据此承诺可配置。

宇树 Go2 官方 [LowState](https://github.com/unitreerobotics/unitree_sdk2/blob/8e1b927586712edd6a4c29d50dd52dff1e163439/include/unitree/idl/go2/LowState_.hpp#L29-L51) 包含 IMU、电机、BMS 和足力等，适合作为待核对的数据范围；其具体型号的字段、频率与 DOSO 的设备能力需要分别验证。
