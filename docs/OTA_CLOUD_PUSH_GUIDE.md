# DOSO 公网云端 OTA 推送全流程指南

> 完整排版版 PDF 文档见 [DOSO_OTA_Cloud_Push_Guide.pdf](DOSO_OTA_Cloud_Push_Guide.pdf)。

---

## 1. 架构总览与三通道设计

DOSO 端云协同 OTA 由以下三层组成：

1. **开发机 / 构建端**：执行策略导出、Golden / Shadow 校验与打包（`tools/export_doso_policy.py`），上传产物至阿里云私有 OSS，并在云端发布 `release` 与下发 `job`。
2. **公网云端（阿里云上海 ECS `47.103.89.137` + 私有 OSS `doso-ota-prod-shanghai`）**：对外通过 Nginx TLS 提供 `https://api-test.dosorobotics.com/ota` 接口（详见 `Kitjesen/robot-cloud`），负责鉴权、任务排队、OSS 签名下载与设备端版本库存上报（`/v1/devices/{id}/status`）。
3. **机载端（RDK S100P `doso-ota` + `doso-brainstem`）**：机载 `doso-ota` 守护进程通过 HTTPS 拉取任务并流式下载校验 SHA256，随后通过本机 `:5054`（`/v1/policies/push`）、`:13146` gRPC（`LocalStatusGrpcServer`）或底座升级完成零重启切换，同时保持 `200Hz/50Hz` 实时闭环无抖动。

### 三条独立 OTA 通道

| 通道 | 云端端点前缀 | 机载/终端接收方 | 核心作用 |
| :--- | :--- | :--- | :--- |
| `policy_package`（运动策略包） | `/v1/releases` + `/v1/jobs` | `doso-ota` -> `doso-brainstem`（`:5054/v1/policies/push`） | 热部署 `.tar.gz` 策略包（`flat` / `stairs` / `recovery` 等），主控制环不停机更新 |
| `brainstem_release`（底座二进制） | `/v1/releases` + `/v1/jobs` | `doso-ota` -> 底座二进制切换与重启 | 升级 `doso-brainstem` 与相关发行包 |
| `android_apk`（手机遥控 App） | `/v1/apps/android/latest` & `/v1/apps/android/releases` | DOSO Android 客户端 App | 分发 App 安装包与强制/可选版本更新元数据 |

---

## 2. 真机全链路实测记录（.13 Dog3）

2026-11-24 在局域网切回状态下，完成开发机 -> 阿里云上海公网云端 -> `.13` 测试板（`192.168.66.13`，RDK S100P，`socketcan`）的全链路真机 OTA 验证：

### 实测环境与构建产物
- **目标设备**：Dog3 `192.168.66.13`，UUID `13131313-1313-4131-8131-131313131313`
- **云端域名**：`https://api-test.dosorobotics.com/ota`（ECS `47.103.89.137`）
- **OSS 对象**：`doso-ota-prod-shanghai/policies/live-ota-20261124/flat_cloud_ota_live_v0_0_1.tar.gz`（HTTP `200`）
- **策略包大小**：`724,464 B`（SHA256 前缀 `71eebf3349fc906b...`）
- **实测推理与校验指标**：`golden_max_err = 3.73e-7`，`warmup_us = 43 µs`

### 两次真实云端推送对照

| 轮次 | Job ID | `manifest.activate` | 耗时与状态流转 | 结果与结论 |
| :--- | :--- | :--- | :--- | :--- |
| **Run 1** | `86696306-3641-49da-8670-4edfc9599968` | `true` | 下载 `724,464 B` -> Golden (`3.73e-7`) -> Shadow -> 落盘均完成 | 安全互锁拦截返回 **HTTP `409`**（`Profile switch requires Grounded state...`），验证机载安全状态机生效 |
| **Run 2** | `57160d2c-a5ba-4bd4-aafe-3b5f80688d02` | `false` | `t=12.18s` `downloading` -> `t=14.21s` **`succeeded`**（执行耗时 `2.03s`） | `doso-brainstem` **PID 保持 `29165`**（0 次重启、`2,047,485` 个 RT tick、`max_mutex_wait_us = 10 µs`、`max_tick_us = 262 µs`、`max_jitter_us = 245 µs`），`slots.flat` 与 `.doso_slots.json` 更新为 `flat_cloud_ota_live_v0_0_1` |

> **生产推送规范**：在 `manifest` 中统一设置 `"activate": false`（先静默落盘预热至对应 slot，待人工或安全状态处于 `Grounded` 时再显式激活），避免非 Grounded 状态触发 HTTP `409`。

---

## 3. 六步标准作业流程（SOP）

### Step 1：本地导出、校验并打包策略

```bash
# 1) 检查模型或策略包目录（支持 .onnx 与策略包目录，输出包含 posture:）
python tools/export_doso_policy.py inspect <POLICY_SOURCE_DIR_OR_ONNX>

# 2) 生成自包含策略包目录（含 policy.yaml 与 Golden 校验基线）
python tools/export_doso_policy.py pack \
  --onnx <CHECKPOINT_ONNX> \
  --name <PACKAGE_NAME> \
  --template height \
  --out-dir dist/<PACKAGE_NAME>

# 也可使用 deploy 一键 inspect + YAML + Golden（加 --dry-run 仅本地生成不推送）：
# python tools/export_doso_policy.py deploy <CHECKPOINT_ONNX> --slot flat --dry-run

# 3) 打成 .tar.gz 归档并记录 SHA256 与字节数
tar -czf dist/<PACKAGE_NAME>.tar.gz -C dist <PACKAGE_NAME>
sha256sum dist/<PACKAGE_NAME>.tar.gz
```

### Step 2：上传制品到阿里云私有 OSS

使用运维工具或受控凭证将 `dist/<PACKAGE_NAME>.tar.gz` 上传至 `oss://doso-ota-prod-shanghai/policies/<RELEASE_TAG>/<PACKAGE_NAME>.tar.gz`，并确保服务端具备对该前缀的读取签名权限（生产环境禁止在脚本或仓库中硬编码 OSS 密钥）。

### Step 3：向云端注册 Release（以 `policy_package` 为例）

```bash
curl -sS -X POST "https://api-test.dosorobotics.com/ota/v1/releases" \
  -H "Authorization: Bearer <API_TOKEN>" \
  -H "Content-Type: application/json" \
  -d '{
    "channel": "policy_package",
    "version": "<VERSION>",
    "artifact_url": "<OSS_OBJECT_URI>",
    "sha256": "<PACKAGE_SHA256>",
    "size_bytes": 724464,
    "manifest": {
      "slot": "flat",
      "activate": false
    }
  }'
```

记下返回的 `<RELEASE_ID>`。如需同步发布 Android App 更新包，使用 `/v1/apps/android/releases` 注册并可通过 `/v1/apps/android/latest` 查询。

### Step 4：对目标机器狗创建下发 Job

```bash
curl -sS -X POST "https://api-test.dosorobotics.com/ota/v1/jobs" \
  -H "Authorization: Bearer <API_TOKEN>" \
  -H "Content-Type: application/json" \
  -d '{
    "device_id": "<DEVICE_UUID>",
    "release_id": "<RELEASE_ID>"
  }'
```

记下返回的 `<JOB_ID>`。

### Step 5：追踪任务状态与远端设备库存

```bash
# 1. 查询 Job 执行状态（queued -> downloading -> succeeded / failed）
curl -sS "https://api-test.dosorobotics.com/ota/v1/jobs/<JOB_ID>" \
  -H "Authorization: Bearer <API_TOKEN>"

# 2. 查询远端设备上报的实时 slot / 策略版本与在线心跳
curl -sS "https://api-test.dosorobotics.com/ota/v1/devices/<DEVICE_UUID>/status" \
  -H "Authorization: Bearer <API_TOKEN>"
```

### Step 6：机载验收与安全回滚

```bash
# 查看 doso-brainstem 健康端点（:5054）与当前 slot 绑定
curl -sS "http://<ROBOT_IP>:5054/health"

# 必要时通过工具执行策略槽位即时回滚
python tools/export_doso_policy.py rollback --admin-url "http://<ROBOT_IP>:5054" --slot flat
```

---

## 4. 关键排查与安全注意事项

- **HTTP `409`（Profile switch requires Grounded state）**：当 `activate: true` 且机器狗未处于 `Grounded` 安全站立状态时，机载安全栅栏会主动拒绝即时切换；建议生产任务均使用 `"activate": false` 静默落盘。
- **HTTPS `443` 与公网路由**：手机热点或跨网段测试时，机载 `doso-ota` 仅需能够出网访问 `https://api-test.dosorobotics.com/ota` 与 OSS 签名地址（`443`），无需在机器狗侧暴露入站公网端口。
- **零重启验证要点**：策略热更成功后，`doso-brainstem` PID 应保持不变，实时控制线程的 `max_mutex_wait_us`、`max_tick_us` 与 `max_jitter_us` 应维持在百微秒级内（`.13` 实测分别为 `10 µs`、`262 µs` 与 `245 µs`）。
- **凭证管理**：所有 `<API_TOKEN>`、签名 URL 及 SSH 私钥统一通过环境变量或受控密钥库注入，禁止写入仓库文档、配置文件或提交历史。
