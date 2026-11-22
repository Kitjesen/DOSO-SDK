# Board-A 实时性能精进与 POSIX SHM 无锁 IPC 开发与部署指南

本文档说明 **DOSO-SDK + Brainstem** 的 Phase 1（全链路热路径降延迟）与 Phase 2（`/dev/shm` 无锁 Seqlock RingBuffer 闭环）的代码修改落点、ARM64 编译打包方式，以及在 Sunrise 运控 A 板（`THUNDER-DEV-001`）上的**零中断分阶段灰度上线与一键回滚步骤**。

---

## 1. 架构与安全边界

- **控制面（低频、强状态机、留在 gRPC `127.0.0.1:13145`）**：
  - `AcquireControl` / `StandUp` / `SitDown` / `Damp` / `Enable` / `Disable`
  - `RobotExternalControl`: `Enter`（颁发 `session_id` + `session_token`）、`Control` 空握手帧与周期性 `input_window` 续租、`Enable`、`Disable`、`Exit`
  - `ControlArbiter` 互斥仲裁与云卓物理遥控器最高优先级抢占保持不变
- **数据面（200Hz–1kHz、零拷贝无锁、可选走 POSIX `/dev/shm`）**：
  - `/dev/shm/doso_proprio_v1`（SPMC 64-slot Seqlock Ring，`592 B/帧`）：Brainstem（Dart FFI 单写）→ `brainstem-dds-bridge` / 本机 RL Runner（多读）
  - `/dev/shm/doso_ext_cmd_v1`（SPSC 4-slot Latest-Wins Ring，`672 B/帧`）：`brainstem-dds-bridge`（单写）→ Brainstem `HardwareExternal`（单读）
- **安全护栏不可绕过**：
  - `HardwareExternal` 每帧校验 `session_id_hash == fnv1a_64(active_session_id)`、当前活跃 `input_window`、严格递增 `sequence`，并强制经过 `RealJoint.validateExternalTargets`（有限值、非负增益、RS04/RS02 扭矩速度边界、Profile 关节限位）；任意越界立即 `interrupt('external_shm_invalid_targets')` 失能电机。
  - 若 SHM 未开启或段尚未创建，自动无缝回退到 gRPC（Latest-Wins `watch` 槽位 + `TCP_NODELAY`）。

---

## 2. 二次开发：改哪里？怎么测？

### 2.1 需要修改的核心位置

| 目标 | 仓库与文件 | 说明 |
| --- | --- | --- |
| 修改共享内存 ABI / 载荷字段 | `DOSO-SDK/bridges/doso-shm-ipc/include/doso_shm_ipc.h` + `src/lib.rs` + `brainstem/thunder/lib/src/telemetry/shm_ipc_bridge.dart` | 必须同步保持精确结构体大小与对齐（单测会强制断言 `592 B` / `672 B`） |
| 调整 Brainstem 推理线程 / 遥测频率 | `brainstem/thunder_brain/lib/src/behaviour.dart` + `brainstem/thunder/bin/thunder.dart` | `_realtimeSessionOptions` 锁定单线程 + Arena；`THUNDER_JOINT_STREAM_MODE` 控制 `snapshot`/`single` |
| 调整 Brainstem 外部 SHM 安全闸门 | `brainstem/thunder/lib/src/external/runtime.dart` + `hardware_external.dart` | `ExternalRuntime.ingestShmFrame` 与 `HardwareExternal` 轮询逻辑 |
| 调整 A 板网关 / DDS 边车转发 | `DOSO-SDK/bridges/brainstem-dds-bridge/src/bridge.rs` + `base_gateway.rs` | `spawn_shm_telemetry_forwarder`、`watch_imu`/`watch_joints` SHM 读、`external_control` SHM 写 |

### 2.2 本地全链路验证命令

```bash
# 1. 编译与压测 doso-shm-ipc（产出 libdoso_shm_ipc.so / doso_shm_ipc.dll）
cargo clippy --manifest-path bridges/doso-shm-ipc/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path bridges/doso-shm-ipc/Cargo.toml --release -- --nocapture
cargo build --manifest-path bridges/doso-shm-ipc/Cargo.toml --release

# 2. 编译与测试 brainstem-dds-bridge 与 DOSO-SDK
cargo clippy --manifest-path bridges/brainstem-dds-bridge/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path bridges/brainstem-dds-bridge/Cargo.toml
cargo test --workspace

# 3. 验证 Brainstem（含真实 Dart FFI <-> Rust 共享内存闭环与 SafetyGate 单测）
cd ../brainstem
DOSO_SHM_IPC_LIB_PATH=../DOSO-SDK/target/release/libdoso_shm_ipc.so \
  dart test -j 1 thunder/test/shm_ipc_bridge_test.dart thunder/test/proto_convert_test.dart
```

---

## 3. ARM64 编译与产物清单（Sunrise 运控 A 板）

在 Sunrise A 板（或 `aarch64-unknown-linux-gnu` 交叉编译环境）编译 3 个产物：

1. **Brainstem AOT 可执行程序**（来自 `brainstem` 仓库）：
   ```bash
   cd ~/brainstem
   dart pub get
   dart compile exe thunder/bin/thunder.dart -o /tmp/thunder
   ```
2. **POSIX 共享内存动态库 `libdoso_shm_ipc.so`**（来自 `DOSO-SDK` 仓库）：
   ```bash
   cd ~/DOSO-SDK
   cargo build --manifest-path bridges/doso-shm-ipc/Cargo.toml --release
   # 产物：target/release/libdoso_shm_ipc.so
   ```
3. **A 板边车 `brainstem-dds-bridge`**（来自 `DOSO-SDK` 仓库）：
   ```bash
   cd ~/DOSO-SDK
   # 若为单板纯运控底盘（无 Board B / 无 CycloneDDS）可直接编；双板需设置 CYCLONEDDS_ROOT
   cargo build --manifest-path bridges/brainstem-dds-bridge/Cargo.toml --release
   # 产物：target/release/brainstem-dds-bridge
   ```

---

## 4. Sunrise A 板分阶段灰度部署步骤（零中断、可秒级回滚）

> **前置硬约束**：切换任何二进制前，必须确认机器狗处于 **趴下状态（`Grounded`）**、`motor_output_enabled=false`、且无活跃遥控/外部控制会话。

### Step 1：按规范目录放置版本化产物（不覆盖 `v0.0.0-baseline`）

```bash
RELEASE=v0.0.2-shm
sudo mkdir -p /opt/brainstem/releases/${RELEASE}/{bin,lib}

# 放入 Brainstem + 依赖库 + libdoso_shm_ipc.so + brainstem-dds-bridge
sudo cp /tmp/thunder /opt/brainstem/releases/${RELEASE}/bin/thunder
sudo cp /opt/brainstem/current/lib/libcserialport.so /opt/brainstem/releases/${RELEASE}/lib/
sudo cp ~/DOSO-SDK/target/release/libdoso_shm_ipc.so /opt/brainstem/releases/${RELEASE}/lib/
sudo cp ~/DOSO-SDK/target/release/brainstem-dds-bridge /opt/brainstem/releases/${RELEASE}/bin/
sudo chmod +x /opt/brainstem/releases/${RELEASE}/bin/{thunder,brainstem-dds-bridge}
```

### Step 2：阶段 A —— 先开 Phase 1（纯 gRPC 优化，SHM 先关）

更新 `/etc/brainstem/brainstem.env`（确保 `THUNDER_JOINT_STREAM_MODE=snapshot`、`THUNDER_SHM_IPC_ENABLED=false`），切换软链并重启：

```bash
sudo ln -sfn /opt/brainstem/releases/${RELEASE} /opt/brainstem/current
sudo systemctl restart brainstem.service
sudo systemctl restart brainstem-dds-bridge.service
```

**阶段 A 验收**：
- `journalctl -u brainstem -n 40` 确认出现 `Joint telemetry gRPC stream mode=snapshot (snapshotPeriodMs=10)`；
- `doso-app`（LAN `:13145`）与 `DOSO-SDK`（`:5052`）均连接正常，遥控起立/行走/趴下正常；
- `brainstem` CPU 与 gRPC 帧率显著下降。

### Step 3：阶段 B —— 打开 `/dev/shm` 遥测与 200Hz MIT 闭环（先不使能电机验证）

1. 在 `/etc/brainstem/brainstem.env` 中打开：
   ```env
   THUNDER_SHM_IPC_ENABLED=true
   DOSO_SHM_IPC_LIB_PATH=/opt/brainstem/current/lib/libdoso_shm_ipc.so
   THUNDER_EXTERNAL_ENABLED=true
   ```
2. 在 `/etc/brainstem/brainstem-dds-bridge.env`（或 `brainstem-standalone-chassis.env`）中打开：
   ```env
   DOSO_SHM_IPC_ENABLED=true
   DOSO_SHM_PROPRIO_NAME=/doso_proprio_v1
   DOSO_SHM_CMD_NAME=/doso_ext_cmd_v1
   DOSO_SHM_POLL_HZ=200
   ```
3. 重启服务并检查共享内存段：
   ```bash
   sudo systemctl restart brainstem.service
   sleep 2
   sudo systemctl restart brainstem-dds-bridge.service
   ls -la /dev/shm/doso_*
   ```
4. **阶段 B 验收**：
   - `ls -la /dev/shm/doso_proprio_v1` 存在且权限正常；
   - `journalctl -u brainstem -n 30` 看到 `POSIX SHM proprioception publisher active on "/doso_proprio_v1"` 和 `RobotExternalControl service enabled (SHM cmd="/doso_ext_cmd_v1", pollMs=5)`；
   - `journalctl -u brainstem-dds-bridge -n 30` 看到 `using POSIX SHM zero-copy proprioception reader`；
   - 客户端调 `capabilities()` 看到 `shm_ipc_enabled: true`，先用**吊装/不使能电机**模式跑通 `low_level` `Enter -> publish_joints -> Close`，确认 `Attached POSIX SHM external command reader` 日志出现且 `doso_ext_cmd_v1` 正常写入，再做真机使能测试。

### Step 4：一键回滚（任一异常 5 秒恢复）

- **仅关闭 SHM（秒级切回 gRPC 数据面，不换二进制）**：
  将 `THUNDER_SHM_IPC_ENABLED=false`、`DOSO_SHM_IPC_ENABLED=false` 后重启两个服务即可。
- **完整回退到基线版本**：
  ```bash
  sudo ln -sfn /opt/brainstem/releases/v0.0.0-baseline /opt/brainstem/current
  sudo systemctl restart brainstem.service brainstem-dds-bridge.service
  ```

---

## 5. 进阶：同板 `doso-rl-runner` 直连 `/dev/shm` + 实机 P99 压测与 IRQ 绑核

当 ONNX RL 策略直接跑在运控 A 板（或同机）时，可开启 `doso-rl-runner --shm-enabled`：
- **控制面**仍走 `RobotClient`（`:5052` → Brainstem `Enter`/`Enable`/`Disable`/`Exit` + `input_window` + 心跳）；
- **数据面**跳过 `:5052` 边车和 loopback TCP，直接 `Brainstem (/dev/shm/doso_proprio_v1)` → `doso-rl-runner (ONNX)` → `(/dev/shm/doso_ext_cmd_v1)` → `Brainstem HardwareExternal`；若 SHM 未打开则自动回退 gRPC。

```bash
# 1. 同板零拷贝 200Hz 运行 RL 策略并输出 P50 / P99 / P99.9 步长与推理延迟
DOSO_SHM_IPC_ENABLED=true \
  ./doso-rl-runner --endpoint http://127.0.0.1:5052 \
  --deploy-yaml /opt/brainstem/data/profiles/deploy.yaml \
  --shm-enabled

# 2. 在 A 板上实测无锁共享内存写/读与 200Hz 定时唤醒抖动（P50 / P99 / P99.9）
cargo run --manifest-path bridges/doso-shm-ipc/Cargo.toml --release --example shm_bench -- --hz 200 --frames 1000

# 3. 锁定 CPU performance 频率并将 CAN/USB-PCAN 中断绑定到 Core 4-5（避开 Core 0-3 相机负载）
sudo /opt/brainstem/libexec/tune-realtime-irq.sh
```

---

## 6. Ubuntu 22.04 ARM64（RDK-S100 / Cortex-A78AE `PREEMPT_RT`）实机验证数据与编译注意事项

### 6.1 实机 `shm_bench` 数据（6 核 Cortex-A78AE @ 2.0 GHz，`6.1.158-rt58 PREEMPT_RT`）

| 场景 | 指标 | P50 | P99 | P99.9 | Max | 撕裂读 |
| --- | --- | --- | --- | --- | --- | --- |
| 10 万帧双读并发压测 | 写入均摊耗时 | **268 ns/帧**（r1=71,705, r2=69,504） | — | — | — | **0** |
| 200Hz 默认 CFS | 定时唤醒抖动 / 往返 IPC | 63.59 µs / 1.18 µs | 72.53 µs / 1.70 µs | 75.98 µs / 1.80 µs | 75.98 µs / 1.80 µs | **0** |
| 200Hz `Core 4-5 + SCHED_FIFO 80` | 定时唤醒抖动 / 往返 IPC | **7.75 µs** / **0.60 µs** | **13.06 µs** / **1.30 µs** | **13.81 µs** / 3.40 µs | **13.81 µs** / 3.40 µs | **0** |
| 1000Hz `Core 4-5 + SCHED_FIFO 80` | 定时唤醒抖动 / 往返 IPC | **4.51 µs** / **0.33 µs** | **14.47 µs** / **1.10 µs** | **17.24 µs** / **1.25 µs** | **17.82 µs** / **1.28 µs** | **0** |

### 6.2 Ubuntu 22.04 ARM64 板上编译注意事项

1. **`protoc` 版本**：Ubuntu 22.04 apt 默认 `protoc 3.12.4` 不支持 `proto3 optional`，请下载官方 `protoc-29.3-linux-aarch_64.zip` 并设置 `PROTOC=/path/to/protoc`。
2. **CycloneDDS 版本**：Ubuntu 22.04 apt `libcyclonedds-dev` 为 `0.8.2`（缺少 `0.10+` 的 `type_information` / `dds_alignof`）。`bridges/brainstem-dds-bridge/build.rs` 已自动检测头文件版本并在 `<0.10` 时平滑降级为单板 Base Gateway 模式；如需双板实时 DDS，请安装 CycloneDDS `0.10+` 并指向 `CYCLONEDDS_ROOT`。
3. **保护 eMMC 根分区**：编译 Rust 时建议设置 `CARGO_TARGET_DIR=/dev/shm/doso-cargo-target`（利用 11GB 内存盘加速编译且不占根分区空间）。


