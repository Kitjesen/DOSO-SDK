# DOSO-SDK 实战教程：一套 SDK 跑通 L2 遥控与 L1 强化学习（含 Sunrise A 板真机部署指南）

本教程面向使用 **DOSO-SDK** 的开发者与算法工程师，说明如何用**同一套 `RobotClient` 接口**：

1. 连接 **单 A 板纯运控底盘（无 B 板）** 或 **A+B 双板整机（含灵途导航）**；
2. 完成 **L2 运动控制与连续速度遥控（Teleop）** 及实时 IMU / 16 关节状态订阅；
3. 完成 **L1 200Hz 强化学习（RL）策略部署**（支持官方零代码 `doso-rl-runner` 与 Python/C++ 自定义推理循环）；
4. 将当前代码安全部署到 **`sunrise`（`THUNDER-DEV-001`，`192.168.66.64`）运控 A 板真机** 上运行与回滚。

---

## 1. 先理解连接拓扑：为什么代码一行不用改？

```text
【形态 A：纯运控底盘（无 B 板，例如当前 sunrise 单板）】
  你的代码 (Python / C++ / Rust / doso-rl-runner)
        │  统一连 http://<机器狗IP>:5052
        ▼
  A 板 brainstem-dds-bridge (standalone 模式, --enable-dds false)
        │  本机回环 127.0.0.1:13145 (local-grpc@bsrl1)
        ▼
  A 板 Brainstem (thunder, 独占 Core 4-5, CAN 驱动 16 电机)

【形态 B：双板整机（A 板运控 + B 板 LingTu 导航）】
  你的代码 (同一套 RobotClient)
        ├── 统一连 http://<B板IP>:5052  ──► L3 导航 / L2 运动 / 传感器
        └── 握手时自动发现 A 板 :5053   ──► L1 low_level() 200Hz 直连 A 板（绕开 B 板 Python GIL）
```

> **核心原则**：外部 SDK 客户端永远只连 `:5052`。在过渡阶段（阶段 1），`Brainstem` 在 `lan-dev` 模式下会同时监听 LAN/热点 `:13145`（兼容现有 `doso-app` / G20 掌机，零中断）与本机回环 `127.0.0.1:13145`（供 `brainstem-dds-bridge` 提供 `:5052`），两者共用同一个 `ControlArbiter` 互斥锁；待 App 迁到 `DOSO-SDK` 后，只需把 `THUNDER_GRPC_TRANSPORT` 切为 `loopback` 即可彻底关闭外网 `:13145`。

---

## 2. 开发机准备（只需做一次）

Python 和 C++17 SDK 共享同一个 Rust 原生内核动态库（`doso_sdk_ffi`），无需安装 gRPC/Protobuf 的 Python 或 C++ 依赖。

### 2.1 编译共享内核动态库

在 `DOSO-SDK` 仓库根目录执行：

```bash
# Linux / macOS
cargo build --manifest-path ffi/Cargo.toml --release
export DOSO_SDK_FFI_PATH="$(pwd)/target/release/libdoso_sdk_ffi.so"

# Windows PowerShell
cargo build --manifest-path ffi/Cargo.toml --release
$env:DOSO_SDK_FFI_PATH = "$PWD\target\release\doso_sdk_ffi.dll"
```

### 2.2 Python 路径配置

```bash
# 将仓库下的 python 目录加入 PYTHONPATH（或 editable 安装）
export PYTHONPATH="$(pwd)/python:${PYTHONPATH}"
```

---

## 3. 教程一：L2 运动控制、实时传感与连续速度遥控

适用场景：站立/趴下/阻尼急停、切换内置步态（平地/爬楼）、发速度指令遥控（`vx, vy, wz`）、读取 IMU 和 16 个电机实时状态。

### 3.1 Python 完整示例（连 `:5052`）

```python
import time
from doso_sdk import RobotClient

ENDPOINT = "http://192.168.66.64:5052"  # 纯 A 板底盘与 A+B 整机都连 :5052

with RobotClient.connect(ENDPOINT, timeout_ms=4000) as robot:
    # 1. 连通性与能力探测（自动区分纯底盘 vs 整机）
    robot.ping()
    caps = robot.capabilities()
    print(f"robot_id={robot.robot_id}, fw={robot.firmware_version}")
    print(f"motion={caps.motion}, low_level={caps.low_level}, board_b_autonomy={caps.board_b_autonomy}")

    # 2. 查看当前运控状态 & 可用内置步态策略
    st = robot.sport().control_status()
    print(f"body_kind={st.body_kind}, motor_enabled={st.motor_output_enabled}, owner={st.source_owner}")
    print("当前策略:", robot.sport().current_profile(), "可选:", robot.sport().list_profiles())

    # 3. 后台实时订阅 IMU 与 16 关节状态（由 Rust Tokio 线程拉流，无 Python GIL 阻塞）
    with robot.observations().telemetry(imu=True, joints=True, stale_after_ms=500) as obs:
        time.sleep(0.1)
        imu = obs.latest_imu()
        joints = obs.latest_joints()
        print("IMU accel:", imu.accel_m_s2, "quat(wxyz):", imu.quat_wxyz)
        print("16 关节角度 q:", [round(x, 3) for x in joints.q])

    # 4. 使能电机并站立（⚠️ 请确保狗周围安全、有人手持云卓遥控器随时接管）
    robot.sport().enable_motors()
    robot.sport().stand_up()
    time.sleep(2.5)  # 等待站立过渡完成

    # 5. 获取控制租约并下发连续速度（SDK + A 板边车自动完成首帧零速确认、租约续期和 20ms 节流）
    with robot.sport().take_control() as teleop:
        # 20Hz 发送前进 0.3 m/s 持续 2 秒，deadman=True 为死手开关
        for _ in range(40):
            teleop.send_velocity(vx=0.3, vy=0.0, wz=0.0, deadman=True)
            time.sleep(0.05)
        # 松开死手或主动停稳
        teleop.hold()
    # 退出 with 自动释放控制权并发送零速

    # 6. 趴下并下电（紧急情况随时可调 robot.sport().damp() 进入软阻尼）
    robot.sport().stand_down()
    time.sleep(2.0)
    robot.sport().disable_motors()
```

### 3.2 C++17 等价示例

只需 `#include "doso/robot_client.hpp"`（头文件仅依赖标准库 + 动态加载 `doso_sdk_ffi`）：

```cpp
#include "doso/robot_client.hpp"
#include <chrono>
#include <iostream>
#include <thread>

int main() {
  auto robot = doso::RobotClient::connect("http://192.168.66.64:5052", "", 4000);
  robot.ping();

  auto caps = robot.capabilities();
  std::cout << "motion=" << caps.motion << " low_level=" << caps.low_level
            << " board_b=" << caps.board_b_autonomy << '\n';

  robot.sport().enable_motors();
  robot.sport().stand_up();
  std::this_thread::sleep_for(std::chrono::milliseconds(2500));

  {
    auto teleop = robot.sport().take_control();
    for (int i = 0; i < 40; ++i) {
      teleop.send_velocity(0.3, 0.0, 0.0, /*deadman=*/true);
      std::this_thread::sleep_for(std::chrono::milliseconds(50));
    }
    teleop.hold();
    teleop.release();
  }

  robot.sport().stand_down();
  std::this_thread::sleep_for(std::chrono::milliseconds(2000));
  robot.sport().disable_motors();
  return 0;
}
```

### 3.3 L2 遥控安全机制（由底层自动兜底）

- **物理遥控器最高优先级**：云卓实体遥控器一动摇杆/按键，立即抢占 `ControlArbiter`，gRPC 租约瞬间吊销。
- **断流看门狗**：客户端程序崩溃或断网超过 `250ms`（可配），A 板 `brainstem-dds-bridge` 自动下发零速并释放租约；`Brainstem` 侧还有 `300ms` 租约硬超时双保险。
- **随时软阻尼**：遇到异常随时调 `robot.sport().damp()`（无需抢租约，只要物理遥控器未占权即生效）。

---

## 4. 教程二：L1 200Hz 强化学习（RL）策略部署

### 4.1 硬件 16 轴关节顺序与 MIT 控制公式

Thunder 轮足机器狗固定为 **16 轴**（前 12 轴腿部位置 PD，后 4 轴足端轮毂速度控制，`kp` 必须为 `0.0`）：

| 索引 | 关节名称 | 控制模式 | 典型参数示例 |
| :---: | :--- | :--- | :--- |
| `0..2` | `FR_hip`, `FR_thigh`, `FR_calf` | 腿部 MIT PD | `JointCmd.leg_pd(q, kp=35.0, kd=1.2)` |
| `3..5` | `FL_hip`, `FL_thigh`, `FL_calf` | 腿部 MIT PD | `JointCmd.leg_pd(q, kp=35.0, kd=1.2)` |
| `6..8` | `RR_hip`, `RR_thigh`, `RR_calf` | 腿部 MIT PD | `JointCmd.leg_pd(q, kp=35.0, kd=1.2)` |
| `9..11` | `RL_hip`, `RL_thigh`, `RL_calf` | 腿部 MIT PD | `JointCmd.leg_pd(q, kp=35.0, kd=1.2)` |
| `12..15` | `FR_foot`, `FL_foot`, `RR_foot`, `RL_foot` | 轮子速度阻尼 (`kp=0`) | `JointCmd.wheel_vel(dq, kd=0.6)` |

单轴电机力矩方程：$\tau = k_p (q_{\text{des}} - q) + k_d (\dot{q}_{\text{des}} - \dot{q}) + \tau_{\text{ff}}$。

### 4.2 进入 L1 的硬性前置条件

`Brainstem` 的 `ExternalRuntime` 强制要求同时满足以下条件，否则 `robot.low_level()` 会安全拒绝进入：

1. 机载 `Brainstem` 已显式开启 `THUNDER_EXTERNAL_ENABLED=true`；
2. 机器人当前处于 **趴下状态（`Grounded`）**；
3. 电机处于 **未使能状态（`motor_output_enabled == false`）**；
4. 当前无活跃的 L2 gRPC 遥控租约、无故障报警。

### 4.3 方式 A（推荐）：用官方 `doso-rl-runner` 零代码部署 ONNX 策略

无需自己写状态机、安全插值或倾覆保护。复制并修改 [thunder_16dof_deploy.yaml](file:///d:/inovxio/brain/DOSO-SDK/rl/doso-rl-runner/examples/thunder_16dof_deploy.yaml)：

1. 将 `policy_path` 指向你从 Isaac Lab / Isaac Gym 导出的 `.onnx` 模型；
2. 按训练配置核对 `joint_ids_map`、`default_joint_pos`、`stiffness`、`damping`、`action_scale`、`obs_scales`、`history_length`；
3. 编译并运行（自动执行 `Passive → FixStand 2s 平滑起立插值 → 50Hz/200Hz ONNX 推理 → 倾覆/超时/Ctrl+C 自动 SoftDamp 软阻尼`）：

```bash
# 编译 RL Runner
cargo build --manifest-path rl/doso-rl-runner/Cargo.toml --release

# 运行（首次务必吊装悬空测试，--vx 0.0 原地踏步/站稳！）
export ONNXRUNTIME_DLL_PATH="/usr/local/lib/libonnxruntime.so"   # Windows 指向 onnxruntime.dll
./target/release/doso-rl-runner \
  --endpoint http://192.168.66.64:5052 \
  --deploy-yaml rl/doso-rl-runner/examples/thunder_16dof_deploy.yaml \
  --vx 0.0 --vy 0.0 --wz 0.0
```

> **提示**：建议将 `doso-rl-runner` 编译为 ARM64 直接跑在 A 板（Core 0–3）或有线直连的 B 板/工控机上，避免 Wi-Fi 抖动触发 `ExternalRuntime` 帧超时保护。

### 4.4 方式 B：用 Python `robot.low_level()` 编写自定义 RL 推理循环

```python
import time
from doso_sdk import JointCmd, RobotClient

ENDPOINT = "http://192.168.66.64:5052"
DEFAULT_Q = [0.0, 0.8, -1.5] * 4 + [0.0] * 4

with RobotClient.connect(ENDPOINT, timeout_ms=4000) as robot:
    # 确保处于 Grounded + 电机下电状态
    st = robot.sport().control_status()
    assert st.body_kind == 1 and not st.motor_output_enabled, f"需先趴下并下电: {st}"

    with robot.observations().telemetry(imu=True, joints=True, stale_after_ms=150) as obs:
        # 进入 L1 外部控制（自动挂起 Brainstem 内置步态 + 完成空帧会话握手 + 获取窗口号）
        with robot.low_level() as ll:
            # 1) 先发送 1 帧安全的当前位姿/软阻尼目标，再 enable 电机输出
            init_joints = obs.latest_joints()
            assert init_joints.complete, "尚未收到完整的 16 轴关节数据"

            safe_cmds = [JointCmd.leg_pd(init_joints.q[i], kp=20.0, kd=1.5) for i in range(12)] + [
                JointCmd.wheel_vel(0.0, kd=0.6) for _ in range(4)
            ]
            ll.publish_joints(safe_cmds)
            time.sleep(0.02)
            ll.enable()

            # 2) 50Hz/200Hz 闭环推理（示例：2 秒平滑插值到默认站姿，再跑你的策略）
            dt = 0.02
            for step in range(100):
                t0 = time.monotonic()
                alpha = min(1.0, (step + 1) / 100.0)
                imu = obs.latest_imu()
                joints = obs.latest_joints()
                if not (imu.fresh and joints.fresh):
                    break  # 传感超时，立即退出并软阻尼

                # 此处可替换为你的 ONNX / PyTorch 策略推理:
                # actions = policy.infer(imu, joints, cmd)
                cmds = [
                    JointCmd.leg_pd(
                        (1.0 - alpha) * init_joints.q[i] + alpha * DEFAULT_Q[i],
                        kp=35.0,
                        kd=1.2,
                    )
                    for i in range(12)
                ] + [JointCmd.wheel_vel(0.0, kd=0.6) for _ in range(4)]

                ll.publish_joints(cmds)
                sleep_s = dt - (time.monotonic() - t0)
                if sleep_s > 0:
                    time.sleep(sleep_s)

            # 退出 with 自动 disable 电机、exit 外部会话，并将控制权归还 Brainstem 内置状态机
```

---

## 5. `sunrise` 运控 A 板真机（`THUNDER-DEV-001` / `192.168.66.64`）上机实操指南

> **安全红线**：操作前机器狗必须**趴下（`Grounded`）、电机未使能（`motor_output_enabled=false`）、无人操作遥控器**；首次测试 L1 RL **必须吊装悬空**，并由专人手持云卓遥控器随时接管急停。

### 第 1 步：更新 `Brainstem` 到本机 `loopback` 版本

电脑切到机器狗同一局域网（`192.168.66.x`）后，执行仓库自带的一键部署脚本（自动做安全前检、ARM64 Core 0–3 隔离编译、原子切链与失败自动回滚）：

```bash
# 先只读检查连通性与安全状态
python scripts/deploy_and_clean_robot.py --check-only

# 确认 Grounded + motor_output_enabled=false 后，构建并部署新版 Brainstem
python scripts/deploy_and_clean_robot.py
```

### 第 2 步：在 `sunrise` 上编译并安装 A 板边车 `brainstem-dds-bridge`

在 `sunrise` 只有单 A 板（无独立 B 板）的当前阶段，以 **独立纯底盘模式（`:5052`，关闭 DDS）** 运行即可，**无需先在板上安装 CycloneDDS**：

1. 将本仓库 `DOSO-SDK` 同步到 `sunrise`（或使用 `aarch64-unknown-linux-gnu` 交叉编译）；
2. 在 `sunrise` 上使用 Core 0–3 隔离编译 Release 二进制（不影响 Core 4–5 实时运控）：
   ```bash
   ssh sunrise@192.168.66.64
   cd ~/DOSO-SDK
   taskset -c 0-3 cargo build --manifest-path bridges/brainstem-dds-bridge/Cargo.toml --release --bin brainstem-dds-bridge
   sudo mkdir -p /opt/brainstem/bridge/releases/v0.1.0/bin
   sudo install -m 0755 target/release/brainstem-dds-bridge /opt/brainstem/bridge/releases/v0.1.0/bin/
   sudo ln -sfn /opt/brainstem/bridge/releases/v0.1.0 /opt/brainstem/bridge/current
   ```
3. 安装 systemd 配置（单 A 板双监听平滑共存：保留 App `:13145` 零中断 + 开启 SDK `:5052`）：
   ```bash
   # 安装 Brainstem 环境文件（lan-dev 双监听：LAN :13145 给 App，127.0.0.1:13145 给本机边车）
   sudo cp /etc/brainstem/brainstem.env /etc/brainstem/brainstem.env.bak-$(date +%Y%m%d)
   sudo install -o root -g sunrise -m 0640 deploy/thunder-dev-001/systemd/brainstem.env /etc/brainstem/brainstem.env

   # 安装并启用 A 板边车服务（注意：单 A 板阶段拷贝 standalone-chassis.env 作为边车环境文件）
   sudo install -o root -g sunrise -m 0640 \
     deploy/thunder-dev-001/systemd/brainstem-standalone-chassis.env \
     /etc/brainstem/brainstem-dds-bridge.env
   sudo install -m 0644 \
     deploy/thunder-dev-001/systemd/brainstem-dds-bridge.service \
     /etc/systemd/system/brainstem-dds-bridge.service

   sudo systemctl daemon-reload
   sudo systemctl restart brainstem.service
   sudo systemctl enable --now brainstem-dds-bridge.service
   ```

### 第 3 步：分阶段真机验收（从零风险到有动作）

1. **阶段 0 — 端口与进程只读验收（零动作）**：
   ```bash
   # 在 sunrise 上确认 127.0.0.1:13145（本机边车）、LAN:13145（App）与 0.0.0.0:5052（SDK）均正常监听
   ss -ltnp | grep -E '13145|13146|5052'
   systemctl is-active brainstem.service brainstem-dds-bridge.service
   ```
   在开发机用 Python 跑只读检查（`ping`、`capabilities()`、`control_status()`、`observations().motors()`、`telemetry(imu=True, joints=True)`），确认 `board_b_autonomy == False` 且 16 个电机均 `online == True`。
2. **阶段 1 — L2 遥控验收（平地空旷区域，手持云卓遥控器待命）**：
   先测 `enable_motors()` → `stand_up()` → `take_control()` 低速点动（`vx=0.15`）→ `hold()` → `stand_down()` → `disable_motors()`，并测试途中拨动云卓摇杆确认物理遥控器立即抢占成功。
3. **阶段 2 — L1 强化学习验收（⚠️ 必须先吊装悬空！）**：
   - 将 `/etc/brainstem/brainstem.env` 中的 `THUNDER_EXTERNAL_ENABLED=false` 改为 `true`，执行 `sudo systemctl restart brainstem.service`；
   - 保持狗趴下且电机未使能，先用 `--vx 0.0` 在悬空状态下跑 `doso-rl-runner` 验证 16 轴转向与关节映射无误、按 `Ctrl+C` 能平稳切软阻尼并退回 `Brainstem`；
   - 悬空确认全部正常后再放落地测试。

### 第 4 步：一键回滚（如遇任何异常）

```bash
sudo systemctl stop brainstem-dds-bridge.service
sudo cp /etc/brainstem/brainstem.env.bak-<日期> /etc/brainstem/brainstem.env
sudo ln -sfn /opt/brainstem/releases/v0.0.0-baseline /opt/brainstem/current
sudo systemctl restart brainstem.service
```

> **未来加装 B 板（LingTu）时如何切换？**
> 只需把 `/etc/brainstem/brainstem-dds-bridge.env` 换成 [brainstem-dds-bridge.env](file:///d:/inovxio/brain/DOSO-SDK/deploy/thunder-dev-001/systemd/brainstem-dds-bridge.env)（监听 `:5053` 并设 `BRAINSTEM_DDS_ENABLED=true`，带 CycloneDDS 重新编译边车），同时在 B 板 `LingTu` 环境变量配置 `DOSO_LOW_LEVEL_ENDPOINT=http://<A板IP>:5053` 即可——**客户端 SDK 代码零改动**。
