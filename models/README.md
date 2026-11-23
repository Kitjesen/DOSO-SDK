# DOSO Thunder 基准模型与声明式策略包 (`PolicyPackage v2`)

本目录存放 Thunder 研发机（`THUNDER-DEV-001`）的 4 套基准 ONNX 模型，以及面向纯 Rust `doso-brainstem` 的**声明式策略包（`packages/`，含极简 `policy.yaml` + `golden.json`）**。观测算子表与关节序双向映射统一收敛在 [`runtime/policy/src/blocks.rs`](../runtime/policy/src/blocks.rs)。

## 1. 声明式策略包清单 (`packages/`)

| 槽位 / 策略包 | 目录 | 输入/输出规格 | 特性 |
| :--- | :--- | :--- | :--- |
| **平地 (`flat`)** | `packages/flat_w5/` | `obs [1, 53]` → `@0 [1, 16]` | 单帧 clipped 53D（17 行 YAML） |
| **楼梯 (`stairs`)** | `packages/stairs_cts_v39/` | `history [1, 530]` (TermMajor×10) + `single_obs [1, 53]` → `actions [1, 16]` | 双输入 CTS + `12 rad/s` 腿部限速（20 行 YAML） |
| **站高调节 (`height`)** | `packages/single_frame_height_v2/` | `obs [1, 58]` → `@0 [1, 16]` | `0.25–0.50 m` (`0.05 m/s`) + 100 帧平滑过渡 |
| **摔倒恢复 (`recovery`)** | `packages/recovery_v89/` | `history [1, 360]` (`6×60`) → `raw_action [1, 16]` + `features [1, 56]` | 6×60 滑窗 + 相位时钟 + 动态 batch |

## 2. 15 行极简 `policy.yaml` 示例

```yaml
schema_version: 2
name: my_isaaclab_flat_v1
kind: locomotion
robot: thunder_v4                # 自动注入站姿 standing_pose 与安全关节上下限
joint_order: isaaclab_joint_major # 自动完成 硬件序 <-> 策略序 双向重排
model:
  file: "model.onnx"
inputs:
  - { tensor: "@0", clip: [-100.0, 100.0], obs: [ang_vel, gravity, cmd_vel, dof_pos_12, dof_vel_16, last_action] }
outputs: { action_tensor: "@0" }
action_decode: { scale_per_group: [0.25, 0.25, 0.25, 5.0] }
gains:
  kp: { hip: 90.0, thigh: 120.0, calf: 120.0 }          # 轮子 kp 恒为 0.0
  kd: { hip: 7.0, thigh: 12.0, calf: 12.0, wheel: 1.0 }
```

## 3. 观测积木块速查表 (`blocks.rs`)

所有 `obs:`（或兼容旧版 `terms:`）既支持**单词简写**，也 100% 兼容 `{ source: ... }` 显式字典；`tensor: "@0"` / `"@1"` 自动按索引绑定 ONNX 输入张量，维度不匹配时直接打印逐块拆解诊断：

| 简写名 | 维度 | 等价算子 (`ObsTerm`) | 默认缩放 / 含义 |
| :--- | :---: | :--- | :--- |
| `ang_vel` | 3 | `GyroRadS` | `gyro_rad_s * 0.25` |
| `lin_acc` | 3 | `AccelMS2` | `accel_m_s2 * 0.1` |
| `gravity` | 3 | `ProjectedGravity` | `projected_gravity` |
| `quat` | 4 | `QuatWxyz` | `[w, x, y, z]` |
| `rpy` | 3 | `RpyRad` | 由 `quat_wxyz` 解算 `[roll, pitch, yaw]` (rad) |
| `cmd_vel` | 3 | `VelocityCmd` | `[vx, vy, yaw] * 1.0` |
| `cmd_height` | 1 | `BodyHeightCmd` | `body_height_cmd` |
| `dof_pos_12` / `dof_pos_16` / `dof_pos_16_zero_wheels` | 12 / 16 | `JointPosRel` | `(q - standing_pose)`（按 `joint_order` 重排） |
| `dof_pos_abs_12` / `dof_pos_abs_16` | 12 / 16 | `JointPosAbs` | 绝对关节角 `q`（按 `joint_order` 重排） |
| `dof_vel_16` / `dof_vel_12` | 16 / 12 | `JointVel` | `dq * [0.05, 0.05, 0.05, 0.05]`（按角色与 `joint_order` 重排） |
| `dof_tau_16` / `dof_tau_12` | 16 / 12 | `JointTau` | `tau * 0.05`（按 `joint_order` 重排） |
| `last_action` / `last_action_12` | 16 / 12 | `LastActionRaw` | 上一拍网络原始动作（按策略自身 `joint_order` 回喂） |
| `"zeros(N)"` / `"const(V)"` / `"phase_clock(P)"` | `N` / 1 / 2 | `Zeros` / `Constant` / `PhaseSinCos` | 零填充 / 常量 / `sin, cos(2π t / P)` |

## 4. `joint_order` 双向关节序速查表

硬件基准序固定为 `thunder_leg_major`（`0..12`: `FR/FL/RR/RL` 髋·大腿·小腿；`12..16`: `FR/FL/RR/RL` 轮）。`compiler.rs` 在观测打包时执行 **Hardware → Policy** 重排，在动作解码前执行 **Policy → Hardware** 逆重排：

| `joint_order` 预设 | 16 维策略序 → 硬件索引 (`joint_map`) | 适用训练框架 |
| :--- | :--- | :--- |
| `thunder_leg_major` *(默认)* | `[0,1,2, 3,4,5, 6,7,8, 9,10,11, 12,13,14,15]` | Thunder 原生腿主序（FR, FL, RR, RL + 4 轮） |
| `isaaclab_joint_major` | `[0,3,6,9, 1,4,7,10, 2,5,8,11, 12,13,14,15]` | IsaacLab 关节主序（4 hip, 4 thigh, 4 calf, 4 wheel；FR/FL/RR/RL） |
| `isaaclab_fl_first` | `[3,0,9,6, 4,1,10,7, 5,2,11,8, 13,12,15,14]` | IsaacLab FL 优先关节主序（FL/FR/RL/RR） |
| `go2_fl_first_leg_major` | `[3,4,5, 0,1,2, 9,10,11, 6,7,8, 13,12,15,14]` | Go2 / LeggedGym FL 优先腿主序（FL, FR, RL, RR + 4 轮） |
| 自定义数组 | `[usize; 16]` 或 `["FR_hip", ..., "RL_foot"]` | 任意自定义 URDF 关节拓扑 |

## 5. 极速模型热部署（`< 2 秒`，零改代码、零重编、零重启）

```bash
# 1) 一键推送验证并热绑定槽位（SHA256 -> ORT 预热 -> Golden 门禁 -> 50帧 Shadow 试跑）
doso-cli --admin-url http://192.168.66.13:5054 push ./models/packages/single_frame_height_v2 --slot height --shadow-frames 50

# 2) 一键秒级回滚到上一验证通过策略
doso-cli --admin-url http://192.168.66.13:5054 rollback --slot height
```

## 6. ONNX SHA256 校验值

- `height/single_frame_height_v2_policy.onnx`: `318bff03d1b765f30553bf5aea85a2b413f58a8a2078eae830a097e6475dffb5`
- `recovery/policy_estimator_actor.onnx`: `1089a94bfe90749b9f4360cab99ffd4ade18a87956e43cf03dbd22f1c0e73016`
