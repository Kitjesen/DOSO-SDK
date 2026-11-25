# RobStride 电机参考文档与固件存档（RS04 / RS02）

## 1. 官方手册与升级说明

- [RS04_Manual_261023.pdf](./RS04_Manual_261023.pdf)：RobStride 04（腿关节，±120 Nm）CAN 2.0 官方使用手册（2026-10-23）
- [RS02_Manual_261023.pdf](./RS02_Manual_261023.pdf)：RobStride 02（轮毂，±17 Nm）CAN 2.0 官方使用手册（2026-10-23）
- [RobStride_Firmware_Changelog_default.pdf](./RobStride_Firmware_Changelog_default.pdf)：RobStride 固件升级说明与版本更新记录（来自 `RobStride/Product_Information`）

---

## 2. 官方固件归档（`RobStride/Product_Information@V26.05.19`）

| 电机型号 | 文件 | 大小 (Bytes) | SHA-256 |
| :--- | :--- | ---: | :--- |
| **RS04**（腿关节） | [rs04_0.4.1.38.bin](./firmware/rs04_0.4.1.38.bin) | `99312` | `a90d83b81455639ddbcf270cc491faa74a354bccbdd7d4322f5cb937bf0351a5` |
| **RS02**（轮毂） | [rs02_0.2.3.36.bin](./firmware/rs02_0.2.3.36.bin) | `98356` | `dd34de7a472f75fb39c34d8f98b15059dadb10d02e9f89ac648d87465e3cc263` |

> 升级建议：同机所有同型号电机尽量保持固件版本一致（12 个 RS04 统一到 `0.4.1.38`，4 个 RS02 统一到 `0.2.3.36`），避免混装旧版本导致的控制/标零行为差异。

---

## 3. 故障记录：未做磁编码校正导致的闭环飞车风险

> [!CAUTION]
> **实机排查结论**：电机如果**没有执行磁编码校正（Magnetic Encoder Calibration）**，FOC 电角度与转子磁极相位可能存在偏差或极性异常，上电使能并给位置/阻尼指令后会出现**正反馈飞车**现象（输出力矩越大，偏离目标角度越快）。

### `.13` 实机排查现象
- 站立（`StandUp`）测试中，同机 `FL_thigh` / `RR_thigh`（固件 `0.4.1.37`）力矩正常收敛在 `-1.5 ~ -9.3 Nm`；
- 而 `FR_thigh`（`can0:2`，固件 `0.4.1.36`）与 `RL_thigh`（`can3:2`，固件 `0.4.1.36`）均出现正向力矩暴冲（`tau` 分别飙升至 **`+96 Nm` / `+108 Nm`**），`RL_thigh` 在 `0.1 s` 内从 `-1.536 rad` 冲至 `-3.415 rad`，触发 `SafetyGate::JointLimitExceeded` 急停。
- 除固件版本混装外，**未做磁编码校正 / 编码器偏置失准**是触发此类闭环正反馈飞车的典型硬件级根因。

### 刷固件 / 拆装电机后的标准校准流程
1. **统一升级固件**（RS04 → `0.4.1.38`，RS02 → `0.2.3.36`）。
2. **空载执行磁编码校正**：确保关节脱开负载或整机吊装悬空后，发送 **Type 5（`Calibration`, `0x05`）** 或通过上位机完成磁编码器校准，等待电机转回 `Reset` 状态。
3. **断电重启确认无报错**（`uncalibrated` / `magneticEncoderFault` / `posInitFault` 均为 0）。
4. **整机姿态下执行 48 帧持久化标零**：发送 16 帧 `Type 6 SetZero` → 16 帧 `Type 18 zero_sta=1 (-π..π)` → 16 帧 `Type 22 SaveData`，重上电验证读数稳定。
5. **吊装小幅度点动复测**：先在吊架上对每条腿低 `kp`/低速测试，确认力矩方向为负反馈后再落地运行 `StandUp` / `SitDown`。

---

## 4. 对应代码实现

- [robostride.rs](../../../runtime/src/hardware/robostride.rs)
- [socketcan.rs](../../../runtime/src/hardware/socketcan.rs)
- [thunder_v4.yaml](../../../runtime/config/thunder_v4.yaml)
