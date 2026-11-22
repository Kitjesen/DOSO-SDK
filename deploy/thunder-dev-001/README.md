# THUNDER-DEV-001 唯一 systemd 配置

> 状态：**Brainstem `v0.0.1` 本地闭环完成；一键部署与语义化清零脚本就绪**。机器狗 `192.168.66.64`，`brainstem.service` 运行于 `/opt/brainstem/current/bin/thunder`（主服务 `:13145` 含 `RobotControl` + `RobotExternalControl` + `RobotRecovery`，本机回环只读状态 `:13146`）。从开发机执行 `python scripts/deploy_and_clean_robot.py`（或上机执行 [converge-release.sh](libexec/converge-release.sh)）即可完成 `v0.0.1` 构建切换、旧目录归档清零，仅保留 `v0.0.1`（当前版）与 `v0.0.0-baseline`（回退基线）。
>
> 收敛完成后，本目录可迁入 brainstem 仓库 `deploy/systemd/`，DOSO-SDK 只保留引用。

## 文件

| 文件 | 安装位置 | 说明 |
|---|---|---|
| [brainstem.service](systemd/brainstem.service) | `/etc/systemd/system/brainstem.service` | 唯一 unit，`brainstem.service.d/` 清空；`CPUAffinity=4 5` 独占核 |
| [brainstem-can.service](systemd/brainstem-can.service) | `/etc/systemd/system/brainstem-can.service` | 替换 `thunder-w5-stairs-can-20261116.service` |
| [can-check.py](libexec/can-check.py) | `/opt/brainstem/libexec/can-check.py` | 4 路 PCAN UP@1Mbps、`txqueuelen=1000`、CPU `performance` 锁频 |
| [brainstem.env](systemd/brainstem.env) | `/etc/brainstem/brainstem.env`（root:sunrise 0640） | 唯一环境文件，替换旧 HAN_DOG 版本 |
| [baseline-20261120/](baseline-20261120/) | — | 现场只读采集：`systemctl cat`、实际进程环境、依赖 sha256 |

## 机器狗规范目录

```
/opt/brainstem/
├── releases/
│   ├── v0.0.0-baseline/         # 上一稳定回退基线（只读）
│   └── v0.0.1/                  # 当前版本（Brainstem v0.0.1，brainstem_api v0.0.1）
│       ├── bin/thunder
│       ├── lib/libcserialport.so
│       ├── manifest.json        # version/commit、构建时间、sha256
│       └── SHA256SUMS
├── current -> releases/v0.0.1   # 升级 / 回滚只切这个软链
├── data/                        # 运行数据，独立版本化（data/manifest.json）
│   ├── operator_modes.json
│   ├── profiles/
│   ├── model/                   # /var/lib/brainstem/model -> /opt/brainstem/data/model
│   │   ├── thunder_flat_w5_model89999_static_b1_arm64.onnx
│   │   └── thunder_cts_moe_v39_model61128_static_b1_arm64.onnx
│   ├── recovery_model/{model.json, policy_estimator_actor.onnx}
│   ├── gestures/{index.json, library.json, *.json}
│   ├── xbox.json
│   └── web/
└── libexec/
    ├── wait-lan-address.py
    ├── can-check.py
    └── voice/send_announcement.py
/etc/brainstem/{brainstem.env, web_control.token, tls/}
/var/lib/brainstem/              # WorkingDirectory (StateDirectory), model -> /opt/brainstem/data/model
/var/log/brainstem/              # THUNDER_LOG_DIR (LogsDirectory)
```

## CPU 物理隔离（Stage 0 单板双域预演）

- **Core 4–5（模拟运控板 A）**：`brainstem.service` 通过 `CPUAffinity=4 5`、`Nice=-20`、`LimitMEMLOCK=infinity` 独占。
- **Core 0–3（模拟算力板 B）**：`lt-camera`、`lt-video-gateway`、`go2rtc`、`askme`、`doso-cloud-link`、`doso-robot-authorizer` 通过 systemd drop-in `CPUAffinity=0 1 2 3` 限制。

## 现场 → 规范路径映射（第一次切换必须逐项一致）

| 用途 | 现场实际来源（2026-11-20） | 规范位置 | sha256 |
|---|---|---|---|
| 主程序（基线回滚） | `deployments/thunder_w5_stairs_gesture_rpc_20261119_31af705/bundle/bin/thunder` | `releases/31af705/bin/thunder` | `10bc0401…b759b` |
| 主程序（Stage 0） | `deploy_staging/w5_stage0_can_drain_20261120/build/stage0/bundle/bin/thunder` | `releases/31af705-stage0/bin/thunder` | `567022c6…dcecd` |
| LD_PRELOAD | `thunder_receiver_20261030/build/thunder-final/bundle/lib/libcserialport.so` | `releases/31af705-stage0/lib/libcserialport.so` | `fbfb06ba…bf33`（与 31af705 自带的相同） |
| ONNX Runtime | `/usr/local/lib/libonnxruntime.so` | 不动 | `0afd69a0…28de` |
| operator_modes | `deployments/…motion_slots_20261117_0e0115b/operator_modes.json` | `data/operator_modes.json` | `00a09e00…b0a` |
| profiles | `deployments/…motion_slots_20261117_0e0115b/profiles/`（35 个文件） | `data/profiles/` | 按文件校验 |
| ONNX 策略模型 | `brainstem_mixed_can_stage_20261023_041903/model/` | `data/model/`（通过 `/var/lib/brainstem/model` 软链） | flat `93d1ec94…` / stairs `d076b543…` |
| recovery model | `deployments/…recovery_20261117_99b8588a/recovery_model/` | `data/recovery_model/` | model.json `bb061cc3…d66d`，onnx `1089a94b…3016` |
| gestures | `~/gesture-library/gestures/` | `data/gestures/` | index `e156655c…4cf`，library `e221885a…104` |
| xbox | `thunder_receiver_20261030/deployment/runtime-data/xbox.json` | `data/xbox.json` | `0bedafac…572` |
| web assets | `thunder_receiver_20261030/deployment/runtime-data/web/`（168K） | `data/web/` | 按文件校验 |
| web token | `brainstem_mixed_can_stage_20261023_041903/han_dog/config/web_control.token` | `/etc/brainstem/web_control.token` | 不打印内容，只比 sha |
| 语音发送 | `~/voice_local_player_test_20261027/send_announcement.py`（**今天 14:58 现场改过，不在 git**） | `libexec/voice/send_announcement.py` | 迁移时采集 |
| CAN 检查 | `deployments/…recovery_20261117_99b8588a/can-check.py`（与 cross_front 的同一文件） | `libexec/can-check.py` | `80f7547d…a9a2` |
| 等待 LAN | `/usr/local/libexec/wait-thunder-lan-address.py` | `libexec/wait-lan-address.py` | `666e3183…56a` |
| 工作目录 / 日志 | `brainstem_mixed_can_stage_20261023_041903/`（logs 246M） | `/var/lib/brainstem`、`/var/log/brainstem` | — |
| 环境变量 | 6 个 EnvironmentFile + 31 个 drop-in 的 Environment 叠加 | `/etc/brainstem/brainstem.env` | 用 `/proc/<pid>/environ` 比对 |

## 当前叠加配置里发现的问题

1. **31 个 drop-in 互相覆盖**：`ExecStart` 被重置 20 次，`EnvironmentFile` 被清空 2 次，最终生效的是 6 个不同目录拼起来的结果。
2. **运行依赖分散在 7 个历史目录**：程序在 11-19 目录、Profile 在 11-17 motion_slots、恢复模型在 11-17 recovery、LD_PRELOAD 和 xbox/web 在 10-30 receiver、日志和 token 在 10-23 stage、语音在 10-27 test。删任何一个旧目录都会让 Brainstem 起不来。
3. **`/etc/brainstem/brainstem.env` 实际未生效**：被 `zz-thunder-receiver.conf` 的 `EnvironmentFile=` 清空，里面是旧的 `HAN_DOG_*` 名字。
4. **`systemctl show` 显示的 `THUNDER_DUAL_POSTURE_CONFIG` 依赖 Unset 才没生效**，属于"看起来有、实际没有"的配置。
5. **Unit 描述写着 "mTLS robot runtime"，实际 `THUNDER_GRPC_TRANSPORT=lan-dev`（明文 + IP 白名单）。**
6. **日志写在 10-23 的 stage 目录**（`THUNDER_LOG_DIR` 未设置，默认 `<cwd>/logs`）。
7. 机器狗上的源码副本是从 Windows 拷过去的 CRLF 文件；内容与 `31af705` 一致（717/717 文件去 CR 后相同）。

## 切换与实机验收记录（2026-11-20 Stage 0 已完成）

前置：机器狗趴下（`Grounded`）、电机未使能（`motor_output_enabled=false`）、无活跃控制权。

1. **备份与预置**：打包 `/etc/systemd/system/brainstem.service*`、`thunder-*-can*.service`、`/etc/brainstem`、`/usr/local/libexec/*thunder*` 至 `/home/sunrise/doso_backups/systemd-stage0-20261120.tgz`，并在 `/opt/brainstem/` 预置 `releases/31af705`（基线回滚）、`releases/31af705-stage0`、`data/` 与 `libexec/`。
2. **原子切换**：`brainstem.service.d/` 归档至 `/etc/systemd/system/brainstem.service.d.archived-20261120/`；安装单一 `brainstem.service`、`brainstem-can.service` 与 `/etc/brainstem/brainstem.env`；禁用旧 `thunder-w5-stairs-can-20261116.service`。
3. **实机验收结果（2026-11-20）**：
   - `systemctl cat brainstem.service` 仅含单一 unit 文件，零 drop-in；`readlink -f /proc/<pid>/exe` 指向 `/opt/brainstem/releases/31af705-stage0/bin/thunder`，`Cpus_allowed_list: 4-5`；
   - `lt-camera.service`（已合入 ARMv8 硬件 CRC32）`Cpus_allowed_list: 0-3`，CPU 占用由 **78.9%** 降至 **1.0%–2.3%**；系统已用内存由 **2791 MiB** 降至 **1965 MiB**；
   - `can0..can3` `qlen=1000`、`1 Mbps`、`ERROR-ACTIVE`，压测期间零新增 `dropped`；
   - `cargo test --locked` 全绿；`read_brainstem` 显示 16 轴电机全部在线无故障；20 秒 `measure_brainstem`（`query_interval_s=5`）通过（IMU `100.000 Hz`，16 轴关节 `99.850–99.950 Hz`，`0` 失败/`0` 超长间隔）。
4. **一键回滚命令**：
   - 仅回退 `thunder` 二进制：`sudo ln -sfn /opt/brainstem/releases/31af705 /opt/brainstem/current && sudo systemctl restart brainstem`
   - 完整恢复旧 systemd 配置：停止 `brainstem`，恢复 `/etc/systemd/system/brainstem.service.d.archived-20261120`、`/home/sunrise/doso_backups/brainstem.service.orig-20261120` 与 `/etc/brainstem/brainstem.env.bak-20261120` 并 `systemctl daemon-reload && systemctl start brainstem`。
