# V1 推进记录

> 历史记录：以下内容保留当时的实现、路径与验证结论。当前主线见[使用说明](../usage.md)和[gRPC 推进记录](../grpc-v1-progress.md)。 Python 预览现归档于 [`archive/python/`](../../archive/python/README.md)。

日期：2026-11-02。阶段：M1 本地开发预览，Python `0.1.0a1`；不是 V1 正式发布。

## 已实现

- 独立的 `DosoClient` 和七个领域入口，源码在 `python/doso/`；不依赖完整 LingTu 安装包，没有第三方运行依赖。
- 状态/健康/就绪/能力/会话查询，软件急停请求，定位状态与重定位，地图列表/保存/事务查询，ProductControl 地图加载及新建图请求。
- map-frame XYZ/yaw 导航目标、按 task_id 查询/取消/等待；Agent instruction 与 run 查询/取消；场景和相机状态查询。
- 命令接纳与任务终态分开；写请求不自动重发，HTTP 重定向不自动跟随，写请求断线及 408/5xx 保留未知结果和 request_id。
- 目标/位姿有限数值检查、坐标系与任务/操作身份检查；矛盾的成功状态和失败证据不会被返回为成功。
- Python 打包、只读连接和导航示例，Ubuntu/Windows × Python 3.10/3.13 的 CI 配置。

## 已有证据

| 验证 | 本地结果 | 范围 |
| --- | --- | --- |
| Ruff | 通过 | 新 Python 源码、测试、示例 |
| pytest | 56 passed，51 subtests passed | Windows / Python 3.13.0；本地 HTTP 假服务和领域测试替身 |
| 实际 Gateway 请求模型 | 通过 | 上述测试内设置 LINGTU_SOURCE，8 次请求经当前 LingTu 实际 Pydantic 模型校验；没有启动服务 |
| wheel/sdist | 已成功构建，最终产物见 dist | Python 安装包，与 C++ 包独立 |
| 干净环境安装 | 已验证独立 wheel 导入 | 新 venv、离开源码目录、清除 PYTHONPATH、隔离模式；不是依赖本机源码导入 |
| CI | 已配置，未在远端运行 | 不据此声称 Linux/Python 3.10 已验证 |

本地测试命令：

```powershell
$env:LINGTU_SOURCE = 'D:\inovxio\brain\lingtu'
& 'C:\Users\99563\miniconda3\python.exe' -m pytest -q
& 'C:\Users\99563\miniconda3\python.exe' -m ruff check python tests/python examples/python
& 'C:\Users\99563\miniconda3\python.exe' -m build
```

## 尚未完成

- 暂停/恢复的 Gateway → GoalService/账本 → navd 接线。
- 站起/坐下的整机 HTTP 入口、带死手/输入窗口的连续遥操作与控制权交接。
- IMU/关节/点云和图像帧的统一 Python 观测接口，巡检与跟随封装。
- 跨版本兼容性表、真实服务闭环、仿真运动、实机运动和开发者试用。
- 主 Gateway 的生产访问保护：当前没有 API Key 认证中间件，SDK header 不能替代服务端鉴权。

下一阶段按 [V1 计划](../v1-plan.md) 的 M2 推进：先关闭服务接线缺口，再扩展公开方法；保留现有 owner、安全边界及停止证据语义。

本轮没有修改 LingTu/Brainstem 运行代码，没有部署或执行机器人动作，也没有提交、推送或发布包。DOSO-SDK 原有 C++ profile 等未提交改动属于此前工作，本轮保留。
