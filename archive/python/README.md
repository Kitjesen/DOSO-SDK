# DOSO Python SDK 开发预览

状态：用户已选择 Rust 为 V1 主实现语言。此 Python `0.1.0a1` 预览已归档，保留为历史接口与测试合同参考，不继续作为 V1 主线推进。新项目请从 [Rust SDK](../../README.md) 和 [V1 计划](../../docs/v1-plan.md) 开始；以下说明记录此 Python 预览自身的用法与限制。

`doso-sdk 0.1.0a1` 是整机 SDK 的首个同步 Python 预览版。它使用标准库访问现有 LingTu Gateway，无第三方运行依赖，不要求在开发者电脑上安装 LingTu 运行时。

这不是正式 V1 发布；已提供客户端接口不代表机器人服务接线、仿真运动或实机能力已经验收。V1 范围和后续阶段见 [V1 计划](../../docs/v1-plan.md)。

## 安装与只读检查

需要 Python 3.10 或更新版本。在本归档目录 `archive/python/` 执行：

```bash
python -m pip install .
python examples/read_status.py --url http://ROBOT_IP:5050
```

```python
from doso import DosoClient

robot = DosoClient("http://ROBOT_IP:5050", client_id="my-application")
print(robot.system.ping())
print(robot.system.readiness())
print(robot.system.capabilities())
print(robot.maps.list())
print(robot.localization.status())
```

构造客户端不联网、不启动产品、不获取控制权。HTTP 地址可以使用现有 HTTPS 反向代理；默认超时为 10 秒，可通过 `timeout` 设置。没有隐式心跳、后台线程或自动重试写请求。

## 当前接口

| 域 | 已实现的客户端方法 | 关键语义 |
| --- | --- | --- |
| system | `ping/state/health/readiness/capabilities/session/product_status` | 原始状态保留；连通不等于就绪或拥有控制权 |
| motion | `emergency_stop/status` | 软件急停回执不证明物理停稳；status 是 navd 的控制/运动证据 |
| localization | `status/pose/relocalize` | pose 返回原始 map odometry，不默认零位姿；须同时检查定位健康与数据时间 |
| maps | `list/save/operation/wait/load/start_mapping/product_operation/wait_product_operation` | 保存和 Product 切换是不同事务、使用不同标识 |
| navigation | `go_to/status/path/task/cancel/wait` | 目标是 map-frame XYZ/yaw，米和弧度；按指定 task_id 等待或取消 |
| observations | `scene/camera_status` | 查询现有服务，不启动传感器或伪造缺失观测 |
| tasks | `send_instruction/get_run/cancel_run` | 指令可能触发机器人运动；run_id 与导航 task_id 不混用 |

站起/坐下、连续遥操作、导航暂停/恢复、IMU/关节/点云订阅、相机帧、巡检/跟随的统一 Python 封装尚未提供。现有 Brainstem C++ 遥测和身体动作接口仍保留原使用方式。

## 导航与结果

以下调用会请求机器人运动。只有目标环境、地图、定位与控制条件已确认后才运行：

```python
receipt = robot.navigation.go_to(3.0, 2.0, z=0.0, yaw=1.57)
if not receipt.ok or receipt.accepted is not True or not receipt.task_id:
    raise RuntimeError(receipt.raw)
print("request:", receipt.request_id, "task:", receipt.task_id)
result = robot.navigation.wait(receipt.task_id, timeout=120)
```

`CommandReceipt` 保留 `ok`、显式 `accepted`（可能未知）、`request_id`、`task_id`、`operation_id`、`run_id` 和 `raw`。`ok` 或 `accepted` 不是动作完成；导航仅在同一 task_id 的任务记录达到 `SUCCESS` 时等待成功。`PAUSED` 不是终态。失败/取消抛 `OperationFailed`。

`robot.navigation.cancel(task_id)` 请求取消指定导航任务。`robot.motion.emergency_stop()` 是整机软件急停请求，与取消单个任务不同。SDK 不自动解除停止、恢复任务或重播速度。

等待超时只结束本地等待，不隐式取消机器人上的工作。每次 HTTP 请求有超时；操作总等待以单调时钟计时。原始响应保留服务端执行证据与拒绝原因。

## 地图与定位

```python
# 保存要求机器人已经处于服务端允许的建图状态。
receipt = robot.maps.save("lab")
if receipt.ok and receipt.operation_id:
    saved = robot.maps.wait(receipt.operation_id, timeout=300)
```

加载地图通过 ProductControl 切到 `nav`。调用前读取 `system.product_status()`，核对当前会话，再显式传入 `expected_product_session_id`；初次无活动会话可显式传空串。SDK 不自动读取并覆盖你的预期会话。

```python
operation = robot.maps.load(
    "lab", expected_product_session_id="CURRENT_SESSION_ID",
    initial_pose={"x": 0.0, "y": 0.0, "z": 0.0, "yaw": 0.0},
)
result = robot.maps.wait_product_operation(operation["request_id"])
```

切换可能重启 Gateway。若失联，保留 request_id，待服务恢复后用 `product_operation(request_id)` 查询；不要换一个 ID 重复提交。当前等待遇到网络错误会显式抛出，不自动恢复。

`maps.start_mapping(expected_product_session_id=...)` 请求进入新的建图 Product；不是跨会话续建。`localization.relocalize(map_name, initial_pose=...)` 只重定位当前活动地图；省略 initial_pose 使用全局搜索，不替代地图加载。重定位默认请求超时为 60 秒，以容纳现有 30/45 秒服务端工作；仍须读取返回结果。

## 错误与身份

- `ApiError`：HTTP 非成功响应，包含 `status_code/payload/method/path/request_id`。写请求遇到 408 或 5xx 时 `submission_unknown=True`，错误可能发生在执行之后，不能据此认定未执行。
- `TransportError`：通信失败，保留 request_id；写请求的 `submission_unknown=True`，不能视为已拒绝，也不会自动重发。
- `ProtocolError`：响应无法解析、类型错误或身份不符；无法确认的写回执保留 request_id 和未知状态。
- `OperationFailed`：查询到指定任务/操作失败或取消，原始证据在 `payload`。
- `TimeoutError`：本地等待到期；机器人任务可能仍在执行。

需要在调用前记录身份时，使用 `from doso import new_request_id`，生成并保存 ID，再作为 `request_id` 传入。也可以从回执/异常读取 SDK 自动生成的 ID。是否允许同 ID 重放以对应服务合同为准，SDK 不自动重发。

## 连接与访问边界

可传 `api_key=`，客户端只将它放入 `X-API-Key` header，用于已配置的访问入口。当前主 Gateway 没有 API Key 认证中间件，发送该 header 不构成已生效的服务端鉴权。默认部署必须处于受控网络，生产访问保护列为 V1 发布前验收项；`client_id` 也不是认证凭据。

SDK 拒绝自动 HTTP 重定向，避免命令重播或把凭据转发到其他地址。TLS 校验沿用 Python 标准库，不关闭证书校验。

## 开发验证

```bash
python -m pip install ".[dev]"
python -m ruff check src tests examples
python -m pytest
python -m build
```

同时有 LingTu 源码时，设置 `LINGTU_SOURCE` 指向该仓库再运行 pytest，会额外用真实 Gateway 请求模型校验 SDK 生成的请求（需该环境已有 LingTu 的开发依赖）。未设置时只跳过这项跨仓库检查，不影响 SDK 独立安装。

测试使用本地 HTTP 服务与领域测试替身，不连接机器人。原 CI 保存在本目录的 `workflow.yml`，仅作历史参考，不再随 push/PR 自动执行；历史验证结果见 [Python 推进记录](../../docs/history/v1-progress.md)。任何尚未执行的 CI、仿真或实机检查不列为已通过。
