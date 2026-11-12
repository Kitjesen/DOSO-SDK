# Rust gRPC V1 推进记录

## Rust 分支与导航运行入口（2026-11-12）

Rust 客户端、七域协议、示例、API 文档、Android Rust 桥接与 CI 由 `rust` 分支维护。[GitHub Rust 主线](https://github.com/Kitjesen/DOSO-SDK/tree/rust)。旧 C++ 客户端保留在 `brainstem/`，历史 HTTP 预览保留在 `archive/python/`；下文“Rust 未提交、未上传”描述本次入库前的检查快照。

导航已有目标提交、任务查询/订阅/等待、暂停、恢复、取消和路线查询/订阅。新增 `examples/navigate.rs`，提供显式地图目标提交到同一任务终态的可运行入口，保留请求与任务身份，拒绝或等待失败时非零退出，不自动重试或取消。[导航运行说明](api/navigation.md#可运行导航流程)。规划和执行仍由机器人侧 LingTu 导航服务负责，正式机端 `5052` 及真实导航到点仍待验收。

本地 Rust 格式、Clippy 与 105 项 SDK 测试通过，另有 7 项示例测试（含 3 项导航参数测试）、7 项 Android 桥接主机测试、2 项协议生成器测试通过。首轮生成器检查因所选 LingTu Python 环境缺少 `grpc_tools` 失败，改用已安装该工具的 Python 后两项通过，未修改 LingTu 依赖。CI 修正 `setup-protoc` 的实际标签，并纳入示例与 Android 桥接主机测试。以上为本地构建与合同证据，不是现场导航或 Android NDK 交叉编译验收。

## 最新现场检查与GitHub状态（2026-11-12 22:55–22:57）

flowfix10的brainstem.service为PID10677 active/running，22:38:51启动，13145在线、13146/正式5052无监听，bodyheight、lt-host、cloud-link均inactive；四CAN UP/1Mbps/berr0，近10分钟严重错误筛选无匹配。云开关false、真实UUID保持，本轮只读检查，没有服务/CAN/运动配置变更。[现场快照](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/current-check-20261112T145556Z.json)

同时SDK/raw 60秒均退出0。SDK60.001253秒completed=true，IMU6,001条/100.015Hz、最大gap104.293ms、源跨度59.996539秒；16轴99.931–99.965Hz、最大逐轴gap108.582ms，关节95,946事件、源跨度59.999781秒。raw两流continuous/passed=true、总passed=true，IMU99.958Hz、最大逐轴gap110ms。本次60秒无2秒消费超时，不代表15分钟长期稳定、严格实时或修复候选已部署；历史失败保留。电脑观察桥SystemInfo三字段均None，真实型号/固件接入与正式机端5052仍缺。[运行/退出](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/current-concurrent-20261112T145643Z.json) · [SDK](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/current-sdk-20261112T145643Z.log) · [raw](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/raw-bdp-on-60s-20261112T145650Z.json)

实际GitHub核对的远端main与codex分支仍为旧C++基线 `0ae4969c`，没有tags/releases。当前Rust七域、Cargo、proto、docs等主要新文件在本地未跟踪、未提交，尚未上传；本库仍是开发预览，不能把本地七域实现或测试说成已发布。用户本轮仅问检查/上传状态，没有执行commit或push。

## GOAWAY 11定位、独立候选与现场flowfix10复测（2026-11-12）

增强Rust探针计划900秒，316.970859秒以Unavailable/GOAWAY 11退出；随后原始直连计划439秒，也约320.52秒同码。两次实际不重合，sdk-900-run的rawConcurrent=false保留，不能描述为同期配对。禁用HTTP代理后BDP on仍约314.29秒复现；BDP off虽等到420秒deadline，源跨度仅约167.27秒，不能作为稳定通过或关闭BDP的修法。旧200.319秒只证明消费超时，现有GOAWAY证据不解释该旧事件。[现场错误与A/B](D:/DOSO/robot-cloud/TESTING.md#goaway-定位候选构建与现场并发复测2026-11-12)

实际grpc5.2.0/http2 2.3.1本地loopback定位keepalive计时比较/相邻间隔未重置，以及响应DATA不通知activity的缺陷；完整候选保留五分钟最小间隔、maxBadPings=2/第三次违规防护及正常BDP。只缩短测试间隔的真实库loopback中，原包2.703秒GOAWAY11、timer-only仍早退、完整候选20.078秒到预定deadline且24PING无GOAWAY。不是以mock推断真实传感器，也不是机端长测通过。[候选与边界](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/rpc-compat-candidate/README.md)

flowfix8及保留现场升级的flowfix10独立候选均已完成ARM64构建、5项只读loopback/mock与零硬件连接/enable=false预检，锁定版本未漂移，两份均未激活。Host deferred模式漏启gRPC另已修复，本地15项通过、尚未部署；13146/正式5052仍未监听，SDK读流仍通过电脑观察桥。[flowfix8构建](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/flowfix8-build/candidate-build.json) · [flowfix10构建](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/candidate-build.json)

用户明确不重启、无需静置，两候选保持未激活。flowfix10的同时SDK/raw 900秒均未通过，旧25.758秒消费超时和42.45秒GOAWAY0保留。后续22:16证据确认旧brainstem.service/PID78009于21:52:02进入Stopping、16轴disable确认，20秒后因停止超时被systemd SIGKILL；同秒bodyheight-ctrl尝试启动、其当前Conflicts包含brainstem.service，未找到操作者记录。21:14 CAN stale属于此前PID70959，不能与本次混因。[停止与服务切换证据](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/failure-141609.json)

22:16曾由bodyheight-ctrl/PID83525提供13145，后续22:24:38已是新boot的brainstem.service、flowfix10/PID1647，bodyheight inactive；13146/正式5052仍无监听。systemd启动时间22:15:07与ps lstart=22:18:26分别保留，不混作同一时间；22:18:41开始的60秒测试按endpoint记录，不能标bodyheight。[新现场与boot](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/failure-142438.json)

该60秒同时读取仍失败：SDK45.604276秒退出1、joint[9]消费idle2001.138ms，8/9/10/11轴约2秒未消费；关节sample_ready不能证明缺轴恢复，其余轴仍有更新且诊断未记录下一样本轴ID。raw IMU99.898Hz、其他12轴约99.8Hz，但8/9/10/11轴在约43秒后消失，源末值56.756–56.763秒，尾部约17.06秒无样本。raw旧completed=true/exit0仅窗口deadline，不能记连续通过；脚本后增continuous/passed与2秒逐轴判定，先纯自检、随后以下10秒真实运行已验证新passed判定，旧60秒JSON/退出原件不改。[SDK](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/current-sdk-20261112T141836Z.log) · [raw](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/raw-bdp-on-60s-20261112T141842Z.json)

内核原件现已直接证实22:19:24 can0 USB Rx aborted(-71)、usb1-2断开/can0 removed，22:19:40重新枚举、22:24仍DOWN；Pcan errno6与其他USB错误另有记录，不能把“EMI?”提示当唯一物理根因，也未核对缺轴到总线的精确映射。三个状态getter均OK与16 online缓存也不证明物理状态或缺轴恢复，须与真实逐轴样本分开。[内核](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/can-kernel-window.log) · [getter](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/current-status-20261112T142639028438Z.json) · [电机缓存](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/current-motors-144123.json)

独立stream-close候选 `doso_sdk_stream_close_flowfix10_20261112T143317Z` 最终串行9项响应/只读测试、1项注入native关闭测试与ARM64构建通过，预检零硬件连接、enable=false、activation=false。首轮并行固定13146测试8过1端口冲突、第二轮fork grpc包示例workspace解析失败保留；最终仅删除fork pubspec中6个未包含的示例workspace声明，版本与依赖不变。候选成功不等于已部署或现场停止问题已修复。[最终构建](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/stream-build.json) · [预检](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/stream-preflight.json) · [首轮失败](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/stream-build-first.json) · [第二轮失败](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/stream-build-second.json)

最新实际SDK/raw双流10秒均退出0、全部16轴重新有样本：SDK10.001492秒，IMU99.985Hz、最大gap295.515ms，各轴99.485–99.585Hz、最大逐轴gap273.034ms；raw两流continuous/passed=true、总passed=true，各轴99.451–99.651Hz、最大逐轴gap297ms。这仅证明该短窗口恢复，不能抹去此前长测和缺轴失败。source_elapsed跨窗口已出现重置，现场程序/boot变化，10秒结果按endpoint/时段记录，不绑定旧PID1647或单一固件身份；本窗口电脑桥SystemInfo三字段均None。[运行记录](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/current-concurrent-20261112T144254Z.json) · [SDK](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/current-sdk-20261112T144254Z.log) · [raw](D:/DOSO/artifacts/robot-cloud/sdk-field-20261112/raw-bdp-on-10s-20261112T144301Z.json)

正式Python启动修复将lifecycle.py的gRPC启动从HTTP defer分离，不把共享host.py他人差异计入本次；候选未替换活动程序/服务配置，测量脚本只清理自有电脑进程。用户不重启限制未被本任务违反，本轮只读/独立构建、未配置CAN、未部署正式机端SDK服务、未启云、未执行运动。

## 单流与双流稳定性复测、真实身份准备（2026-11-12）

本节保留此前flowfix8窗口：当时Brainstem PID41773 active/running、SHA256 `a2398ada37abb4bd6f25227335cb21472c0b200aba7079f127cac31b356413fd`，13145在线、13146/5052未监听、lt-host未安装，SDK观察桥在电脑回环。后续现场版本/用户限制与新复测见上节，不将此快照当当前在线证明。[目标快照](D:/DOSO/artifacts/robot-cloud/sdk-stability-20261112/robot-110510.json)

`measure_observations <endpoint> [seconds] [both|imu|joints]`已提供三种模式，原用法默认both兼容；只订阅、统计和检查所选流/轴，保留固定墙钟、2秒消费超时失败、EOF/错误、实际source_elapsed、窗口首尾最大gap和drop取消，不自动重连。初版示例2项测试、cargo check、构建、rustfmt、帮助检查通过，库API与依赖未改；后续诊断增强见下文。[用法](api/observations.md#固定窗口测量实际接收频率)

后续仅增加已有字段输出：握手后一次SystemInfo三字段，每项原统计行保留并追加独立source_first_ns/source_last_ns行，缺失保持None，便于重复连接比较源时间前进，无新状态或时钟推断；rustfmt与构建通过，未重跑无关测试。

真实IMU单流60.012434秒收到6,001条、99.996Hz，最大gap60.738ms、源跨度59.998687秒；关节单流60.001822秒收到95,940事件，各轴99.914–99.947Hz，最大逐轴gap152.323ms、源跨度59.999019秒。两个60秒窗口完成，不能推为长期稳定或抹去此前IMU-only20帧停流记录。[IMU单流](D:/DOSO/artifacts/robot-cloud/sdk-stability-20261112/rust-imu-60s.log) · [关节单流](D:/DOSO/artifacts/robot-cloud/sdk-stability-20261112/rust-joints-60s.log)

双流计划300秒，在200.319451秒触发2秒消费超时失败，`completed=false`：IMU 19,833条、源跨度198.315270秒，关节317,130事件、源跨度198.331038秒。[原始失败](D:/DOSO/artifacts/robot-cloud/sdk-stability-20261112/rust-both-300s.log) 随后短时原始直连正常，服务PID/二进制哈希未变；该失败与日志原措辞保留，不归因为服务重启或已确定的SDK/网络缺陷。

后续SDK与原始流并行300秒，SDK300.001840秒completed=true：IMU30,001条、100.003Hz、最大gap184.444ms、源跨度299.992651秒；关节478,138事件、1,593.784条/秒，各轴29,880–29,888条、99.599–99.626Hz，最大逐轴gap638.590ms、源跨度299.024850秒，较墙钟约少1秒，平均近100Hz不等同无积压。[SDK五分钟](D:/DOSO/artifacts/robot-cloud/sdk-stability-20261112/rust-both-paired-300s.log) 同期原始直连IMU99.979Hz/最大gap187ms，关节1,598.633条/秒、各轴29,973–29,982条、源跨度299.964362秒，整条关节事件流最大gap156ms，没有超过500ms的事件间隔；总体流gap不当作逐轴gap。[原始五分钟](D:/DOSO/artifacts/robot-cloud/sdk-stability-20261112/direct-sync-300s-20261112T111332Z.json) 本次没有重现2秒消费超时，但首轮失败、源跨度落后和逐轴长gap均未消除，故障未定位、稳定验收仍未通过。

后续诊断确认，2秒watchdog表示消费者未处理样本，不能证明上游停发；未找到200秒硬编码截止，但完整flowfix8源/依赖未核对。示例保持阈值、biased顺序、非零失败、不重连，新增trigger/消费idle/最后源时间及失败后单次零等待read分类（sample_ready附next_source_ns、eof、rpc_error、no_sample_ready），不计入原统计、不改判成功；4项单测、check/build、格式和帮助通过。本地真实SDK→生产只读Gateway→模拟Brainstem：baseline3秒退出0，仅暂停自有Rust客户端2.25秒时源计数继续（IMU325→552、关节4624→7800），恢复后约2.3秒消费超时、退出1，两流均sample_ready。该机制可复现，不证明历史根因，本轮无新真机长测。[结果](D:/DOSO/artifacts/robot-cloud/stream-idle-20261112/result.json) · [暂停日志](D:/DOSO/artifacts/robot-cloud/stream-idle-20261112/paused-consumer.log) · [完整诊断](D:/DOSO/artifacts/robot-cloud/stream-idle-20261112/README.md) 下一步先分层对照再真机验证，随后机端SDK及真实型号/固件元数据，最后证书、公钥/ACL和获许可云状态。

用户选定THUNDER-DEV-001，真实库存UUID `d82a831a-2e86-4b46-975b-3d42473583b7` 已登记并写入机端agent.env，非demo/非模拟、未认领，旧样例已备份；重复执行changed=false、UUID未旋转，云开关保持false且未重启。机端独立TLS私钥/CSR与Ed25519状态签名key已生成，CSR CN与UUID一致、私钥未下载；device.crt签发安装、云端Gateway公钥/ACL和制造认领密钥尚未完成，不能将状态签名密钥混作制造身份。[身份准备与完整边界](D:/DOSO/robot-cloud/tools/deploy/IDENTITY.md#首台研发机身份准备2026-11-12) 最后三次both模式10秒短重连均退出0、completed=true，电脑桥robot_id均为真实UUID，IMU和关节事件的下一连接source_first_ns均超过上次source_last_ns；原件见 [TESTING](D:/DOSO/robot-cloud/TESTING.md#真实身份准备与-sdk-稳定性复测2026-11-12)。但robot_version/firmware_version均为None，真实机端release接线仍缺。库存model_code「THUNDER 01」不能冒充SDK型号字段已配置，Brainstem flowfix8构建名/SDK版本不作固件值；字段一致也不视为机端Host/5052已安装。三次短窗口完成不消除首轮长读失败，本轮不代表长期稳定、机端完整SDK服务、客户认领、云端真实上报或运动通过。

## 真实 IMU/关节只读链路与转发瓶颈定位（2026-11-12）

**此前阶段快照（17:40，北京时间，当前已为上节flowfix8）：** 当时flowfix7b PID35151 active/running、13145监听，17:37:20启动。程序为 `/home/sunrise/deployments/thunder_dual56501_flowfix7b_20261112/bundle/bin/thunder`，恢复/部署非SDK任务操作。[快照](D:/DOSO/artifacts/robot-cloud/live-sdk-20261112/current-094036.json) single-thread channel选项接入后45项/Ruff通过，当时默认代码20秒IMU99.646Hz、每轴93.397–93.447Hz、逐轴最大gap280.441ms，已读通但未稳定达到100Hz；历史证据不替代上节长读结果。

恢复前默认代码Rust20秒曾建流超时，紧随原始直连约4秒的IMU/关节均为0样本。[默认代码失败日志](D:/DOSO/artifacts/robot-cloud/live-sdk-20261112/rust-observations-default-20s.log) · [直连0样本](D:/DOSO/artifacts/robot-cloud/live-sdk-20261112/direct-sync-4s.json) 该失败保留为时序记录，不能先行判为SDK缺陷或最终默认实现通过。

[17:36故障快照](D:/DOSO/artifacts/robot-cloud/live-sdk-20261112/current-093608.json) 当时为failed/failed、PID28915退出、13145无监听；记录17:03:24曾重新启动（非本轮SDK任务操作），17:18云卓H使能、L1接纳及随后模式切换失败，17:33:18停止时出现物理disable未确认。17:40曾恢复为flowfix7b，当前flowfix8另见上节。不能根据这些记录将服务失败归因于SDK，也不能将该次disable错误视作物理关闭确认。此前60秒近100Hz结果仍是当时的有效测量，不自动代表新版通过相同验证。

真实Rust SDK此前已通过电脑回环上的Python观察服务读取机器人 `192.168.66.64:13145`，不再只有fake Brainstem的本地合同证据。LingTu `tools/sdk/observe.py` 仅注册Handshake、WatchImu、WatchJoints，复用生产观察adapter和Gateway handlers，不启动硬件Host/driver、不注册运动或产品切换RPC。测量时该主接口为明确的LAN开发明文模式、电脑IP已在白名单；探针须显式指定 `--lan-development`，使用只读源的 `LINGTU_OBSERVATIONS_TRANSPORT=lan-dev`，仅接受字面RFC1918 IPv4、拒绝混配TLS。默认远端mTLS要求保留，没有自动降级。

新增只读示例 `examples/measure_observations.rs`：参数为 Gateway endpoint 与可选秒数（默认10），并发订阅 IMU/关节，以同一墙钟窗口统计消息数、Hz、每关节条数、最大接收间隔、线加速度存在数及实际源相对时间跨度；无源时间时保持未知。任一源/轴连续2秒无数据、提前 EOF 或流内错误均失败，结束时丢弃两个订阅，不重连掩盖停流。支持 `DOSO_API_KEY` 与现有 HTTPS 信任根，私有 PEM CA 通过示例的 `DOSO_TLS_CA_FILE` 装入既有 `Config.tls_ca_certificate`。示例统计单测1项、构建、`cargo check --locked --example measure_observations` 与 `--help` 通过；Python 本地相关45项通过。用法见 [观察 API](api/observations.md#固定窗口测量实际接收频率)。

首轮真实 Rust 测量窗口为20.001201秒：IMU 2,002条、100.094Hz，源时间跨度20.009644秒，最大接收间隔73.569ms，线加速度存在0/2,002。关节事件24,921条、1,245.975条/秒；各轴1,557–1,558条、77.845–77.895Hz，事件源时间跨度15.581882秒，逐轴最大接收间隔最高75.634ms。[原始测量汇总](D:/DOSO/artifacts/robot-cloud/live-sdk-20261112/rust-observations-20s.log)

`completed=true` 只表示窗口完成且未触发2秒空闲/断流条件，**不代表关节100Hz目标已通过**。首轮关节源时间在20秒墙钟里只前进约15.58秒，属于未达目标的原始记录；此前IMU-only仅收到20帧后停流的结果也保留。

flowfix5同一Python环境直连真实源的20.031秒对照收到IMU 1,999条、99.795Hz、关节31,902事件、1,592.631条/秒，关节源时间跨度19.984995秒，说明当时源速率正常、瓶颈位于转发路径。[flowfix5对照归档](D:/DOSO/artifacts/robot-cloud/live-sdk-20261112/direct-sync-flowfix5-20s.json) 原共享文件名 `direct-sync-20s.json` 被新版运行覆盖，旧数据从此前工具原始输出恢复并保留来源说明，不能当作未改动原始文件。启用gRPC现有single-thread unary-stream环境开关后，Rust20秒达到IMU99.858Hz、每轴99.758–99.808Hz；这轮有并行离线负载，作为A/B定位记录，flowfix5阶段主要证据采用后续干净60秒。[20秒A/B日志](D:/DOSO/artifacts/robot-cloud/live-sdk-20261112/rust-observations-single-thread-20s.log)

flowfix5干净60秒实测窗口60.002805秒：IMU 5,999条、99.979Hz，最大接收间隔70.889ms、源跨度59.979214秒，线加速度存在0/5,999；关节95,923事件、1,598.642条/秒，各轴5,994–5,996条、99.895–99.929Hz，逐轴最大间隔最高76.383ms，关节事件源跨度59.989973秒。[60秒原始日志](D:/DOSO/artifacts/robot-cloud/live-sdk-20261112/rust-observations-single-thread-60s.log) 此设置下当时链路近100Hz、未见明显持续积压，但最大接收间隔仍达数十毫秒，不是严格实时或长期稳定证据，也不能证明硬件采样同步；该记录不作为当前新版性能结论。

最终默认代码、关闭环境开关后在flowfix7b完成20.000744秒：IMU 1,993条、99.646Hz，最大gap120.424ms、源跨度19.919941秒，线加速度存在0/1,993；关节29,900事件、1,494.944条/秒，每轴1,868–1,869条、93.397–93.447Hz，最大逐轴gap280.441ms、源跨度18.699046秒。[最终默认实现实测](D:/DOSO/artifacts/robot-cloud/live-sdk-20261112/rust-observations-default-flowfix7b-20s.log)

新版原始直连则收到IMU 1,993条/20.047秒、99.416Hz，关节31,871事件/20.031秒、1,591.084条/秒，各轴1,991–1,992条、约99.4Hz、源跨度19.929782秒。[flowfix7b直连归档](D:/DOSO/artifacts/robot-cloud/live-sdk-20261112/direct-sync-flowfix7b-20s.json) 因此本阶段代码与真实读取已完成，但双gRPC边界转发在变动负载下仍有吞吐不足/积压风险，100Hz目标尚未稳定达成，不能将本地45项/Ruff通过或 `completed=true` 解释为性能目标全部通过。

该阶段SDK监听服务在电脑 `127.0.0.1:50051`，不是机端SDK Gateway部署。此前flowfix5确认5052/13146未监听；flowfix7b的`ss`只取前15行，当时不能排除两个端口，后续flowfix8完整检查见上节，机端完整SDK服务仍未完成部署验收。cloud-link真实服务用户关闭态曾输出 `cloud_off` 且inactive/disabled；该次UUID仍为模板、设备密钥未准备，后续真实库存与机端身份/密钥已更新，远程云保持关闭。[阶段证据](D:/DOSO/robot-cloud/TESTING.md#真实机器狗关闭态与本地-sdk-观测2026-11-12) SDK任务没有发送动作或完成云端真实状态验收，现场云卓操作日志不属于SDK运动测试。

`13146`的 [补丁候选](D:/DOSO/artifacts/robot-cloud/live-sdk-20261112/local-status-candidate/README.md) 已准备，复用既有native状态RPC，仅提供回环GetControlStatus，不含IMU/关节。只通过flowfix5旧基线锚点和隔离适用性检查，未编译、部署或验证生命周期，候选准备时匹配native entry仍不完整；现场已切flowfix7b，旧候选不能直接部署，须按新版重新核对，不能当作已可运行或已安装的替换二进制。

## 公开系统信息与云权限边界（2026-11-12）

System 的 State、Health、Readiness、Capabilities、Session、ProductStatus 六个 gRPC 快照改为明确公开字段投影，内部端点、进程证据、文件路径和运行配置不再原样返回。内部诊断 owner 未变，Readiness 的 false/null/缺省与 Product 切换精确 CAS 保留。公共投影实现位于 LingTu `src/gateway/rpc/system_view.py`，handler 共用该投影。

Rust `system.product_status()` 现在返回 proto 定义的 `ProductStatus`，包含 available、status、product 与必要的 product_session_id，代码在 `src/system/product.rs`。gRPC 线协议方法仍返回公开 Snapshot，Rust 侧执行类型化读取与必要格式检查。开发预览调用方需改为直接读字段，地图切换协议与校验语义不变；只读示例默认不打印内部校验值。

未预期 RPC 异常使用固定 INTERNAL/internal_error，traceback 仅写服务日志；已有业务错误和写回执语义保持。设备无云管理权、云端禁止远程运控、本地 SDK 保留操控的契约已同步，云状态链仍固定 GetControlStatus。客户数据与云侧实际回归统一见 [robot-cloud TESTING](D:/DOSO/robot-cloud/TESTING.md)。

Rust全量105项通过（8单元、84gRPC、2格式、11文档）；Clippy通过。修正上一轮位置测试的 then/then_some lint 后，3项位置专项复测通过，计数与全量重叠。新增11项真实Python gRPC公共视图测试及69项相邻回归通过，共80项。真实Rust `read_status` CLI与生产Python handlers回环退出0，确认类型化ProductStatus、false就绪值和公开能力，内部marker未进入输出；输入为明确模拟的诊断源字典。[脱敏结果](D:/DOSO/artifacts/robot-cloud/public-sdk-20261112/result.json) · [复现命令](D:/DOSO/artifacts/robot-cloud/public-sdk-20261112/commands.ps1)

本轮未部署到机器人或云服务器，其他领域的业务结果与诊断格式仍按各自接口合同解释。字段投影不作为客户人员隔离已全部验收的证明。

## 地理位置查询与订阅（2026-11-11）

新增 `localization.location()` 与 `watch_location()`，消息由 `proto/messages/localization.proto` 定义，Rust 实现在 `src/localization/location.rs`。LingTu 的 `gateway/rpc/location.py` 复用已有 `geography.geographic_location()`，不重复打开 GNSS 接收器；WatchLocation 复用共享订阅生命周期，每秒读取一次现有快照。WGS84 经纬度与 map 位姿分别提供，Ready 才携带 fix，保留 GNSS 源 UTC 时间、可选精度和源年龄；无 fix、过期、缺文件和无效分别报告，恢复后继续同一流。不启用云采集。

Rust `cargo test --locked` 通过 8 项单元、82 项 gRPC、2 项格式与 11 项快速入门文档测试，共 103 项。Python 受影响 42 项通过，包含 11 项新真实 gRPC 测试；Ruff 通过。覆盖原始时间、未知/零精度、缺 fix 与过期、1Hz 快照与恢复、鉴权、共享名额和取消，以及 SDK metadata、订阅总超时与旧服务 Unimplemented。首次 Python 运行的测试夹具曾替换全局时钟，干扰 gRPC deadline，改为仅替换 geography 自己的时钟引用后通过；产品时钟逻辑未改。

实际跨语言回环使用生产 LingTu gRPC server 与已构建的 Rust `watch_location.exe`，输入为明确模拟的临时 GNSS 文件。约 4 秒收到 Ready、Ready、NoFix、Stale、Ready，源时间与未知精度保留。实测发现当代 epoch 整值乘纳秒损失 64ns，修复为整数秒与小数秒分别计算后，11 项位置专项与 Ruff 再通过。测试 server 主动停止后 Rust 按预期收到 Unavailable 退出，端口与进程已清理。[结果与原样输入](D:/DOSO/artifacts/robot-cloud/location-20261111/cross-language.json) · [复现命令](D:/DOSO/artifacts/robot-cloud/location-20261111/commands.ps1)

使用说明见 [地理位置 API](api/localization.md#地理位置)，运行示例为 `cargo run --example watch_location -- http://<robot>:5052`。本轮为本地实现与协议验证，未在真实机器狗部署，真实 GNSS、现场频率与公网数据上报另验。

## 电机门与Profile切换（2026-11-07）

新增 `motion.enable_motors/disable_motors/profile/switch_profile`。电机写操作和Profile切换复用命令journal、native body事务、DDS与driver，Profile名称进入完整请求去重。共享Gateway业务操作从 `services/posture.py` 扩展为 `services/body.py`，站坐、电机门与Profile命令保留同一套回执与未知结果处理。

Enable只打开输出门，Disable撤销lease并关闭输出，不提交身体动作。Disable可在软件急停或控制循环恢复hold下执行，仍检查操作者与物理owner边界。正常活动任务需先结束。公共Profile切换要求Grounded、电机关闭和控制权释放，目标取自独立 `profile()` 查询，ACK后继续独立读取实际current。Profile读取复用配置好的Brainstem只读通道和凭据。

内部自动楼梯切换补齐disable、关门读回、switch、Profile读回、enable、开门读回和stand阶段，仅恢复入口明确已使能的电机门。入口已禁用时等待显式命令，故障与外部禁用后保持hold。Grounded且无控制lease的正常入口可直接进入失能阶段。

Rust全量95项通过，Clippy与格式检查通过，motion手册11段Rust示例编译通过，协议生成器2项通过。Gateway整合137项通过，其中body专项41项。Brainstem只读源及已有读取回归25项通过，native Python adapter/commands79项与operator状态6项通过。原生navd、client、driver构建通过，真实本地DDS command_roundtrip与ingress专项通过，driver CTest 8/8通过，包含DDS到本地mock Brainstem的Stand/Disable/SwitchProfile/Enable及Go2回归。

所有验证使用本地链路，未部署或连接机器人。逐电机物理输出、目标设备模型加载和现场切换仍需验收。既有枚举冻结测试表仍遗漏 `InspectionTaskState.WAITING_NEXT_LOOP` 与 `SensorControlKind`，本轮新增三个命令值已同步冻结表，该基线问题单独保留。

## 控制权、恢复与已知故障（2026-11-07）

新增 `motion.claim_control`、会话 `claim/release_control/control_state`、全局 `control_state/watch_control_state`、`motion.resume` 与 `system.faults/watch_faults`。权限复用同一个Teleop会话和native owner，读取通过原生DDS状态、C ABI和既有nav.commands能力接入。恢复由HTTP与gRPC共用同一业务操作和journal。缺失申请或恢复ACK保留未知提交，明确拒绝保留原始原因。

控制权与故障提供目标最高50Hz快照，中间状态可合并。权限源过期保留未知，网络传输年龄未知。native lease为1秒，无后台保活。故障覆盖driver控制、driver健康和navigation控制循环三个来源，并逐帧列出可用与不可用来源。完整事件、更多故障来源和设备能力继续见V1缺口清单。[接口手册](api/README.md) · [剩余工作](v1-plan.md#对标后的能力缺口)

Rust全量单元、gRPC、格式与快速入门文档测试通过，后续申请结果语义修正的5项专项和Clippy通过。API手册29段Rust示例编译通过，协议生成器2项通过。Gateway整合回归分别133项和111项通过，最后Teleop与控制专项47项通过，部分测试组有重叠。native Python适配回归46项通过，原生client与test_nav_client构建通过，实际本地DDS状态回环CTest通过，受影响Python Ruff通过。

本轮仅本地代码与测试，未部署或连接机器人。目标平台的控制交接、链路频率和实际运动仍待验收。

## API手册与厂商缺口整理（2026-11-07）

新增`docs/api/`手册入口、连接公共约定及七个领域页，共9页。按当前源码核对90个手写公开方法，包括连接/领域访问、命令/查询/订阅、错误访问和本地消息辅助方法。每个领域说明参数、单位、返回、前置条件、失败与示例，待实现功能在V1计划的缺口表单独维护。

结构参考智身GEN2与宇树Go2固定提交的官方API资料，具体行为采用DOSO当前Rust/gRPC实现。README和快速入门链接到API手册，原快速入门保留调用流程，逐方法参考集中在领域页。

27段新增Rust示例通过`rustdoc --test`的no_run编译检查，示例没有运行设备操作。公开方法签名覆盖检查无遗漏，9页及相关入口的245个本地链接和锚点检查通过。快速入门原有11项Cargo文档测试也通过。本次仅变更文档，实机部署与动作验收继续保留现有边界。[API手册](api/README.md) · [能力缺口](v1-plan.md#对标后的能力缺口)

## 里程计频率确定为10Hz（2026-11-06）

按产品确认，`motion().watch_odometry()` 每100ms推送一次，目标最高10Hz，与当前定位源默认状态导出频率对齐。共享缓存订阅器接受所属调用方传入的周期，里程计传入0.1秒。源失效、取消和共享流配额沿用既有处理。速度反馈的新样本更新仍受定位源约束，当前默认约10Hz。

相关Gateway反馈与订阅回归53项通过，包含正常RPC入口按100ms周期、扣除处理耗时和配额释放的验证；Ruff及Rust格式检查通过。此前速度/里程计首轮联调的约66Hz为当时缓存推送结果，保留历史记录。


## 速度与里程计读取接线（2026-11-06）

补齐四个实际读取入口，单次查询和服务端流共用Gateway已有定位里程计缓存。

| SDK文件 | 公开方法 | RPC |
| --- | --- | --- |
| `src/motion/velocity.rs` | `velocity()`、`watch_velocity()` | `Velocity`、`WatchVelocity` |
| `src/motion/odometry.rs` | `odometry()`、`watch_odometry()` | `Odometry`、`WatchOdometry` |

`VelocitySample`提供body坐标系三轴线速度m/s、三轴角速度rad/s。`OdometrySample`提供map→body位置和完整单位四元数，并携带同帧body速度。两类消息追加`source="localization"`、可缺省`received_age_ms/fresh`，保留原字段号。

当前real链路使用native SLAM的估计反馈。native status adapter明确提供六轴twist，Host Odometry新增`twist_available`，默认false，生产者明确有反馈时才设true。Gateway原缓存补wx/wy，新接口拒绝旧缓存缺轴、非有限、失效隔离、过期和定位丢失，真实静止0正常交付。配置的默认Twist不作为可用反馈。现有源ts没有明确时钟域，SampleTime保持缺失，接收年龄单独表达。

缓存订阅目标最高100Hz，本地Gateway约6秒的回环测得速度66.535Hz、里程计66.880Hz，接收间隔p95约17ms。当前real默认`status_json_hz=10`，Python桥接最短50ms轮询，源新样本默认约10Hz，高频缓存推送会重复同一源样本。每秒50份新反馈尚需调整源端导出链路并验收。

Rust完整8项单元、65项gRPC、2项消息格式及11项文档测试通过，Clippy通过。Gateway新39项kinematics及原13项telemetry通过。Host消息、SLAM适配、Gateway frame与runtime状态相关165项通过。协议生成器2项通过，受影响Python Ruff通过。跨语言联调使用真实Rust SDK、GatewayModule和native SLAM状态适配器，模拟SLAM源输入，九组检查全部通过，覆盖四RPC的非零六轴/姿态、合法静止0、缺失/过期/丢失、活动流失效、取消后重开和全部配额释放。5秒双流各332帧，接收约66.384Hz；模拟源51次更新，约9.979Hz，接收年龄0–108.989ms。证据位于`.local/evidence/kinematics/results.json`，示例入口为`examples/read_motion.rs`。另105项Gateway RPC整合回归通过，和上述专项有重叠，分别记录。没有连接真机或部署服务。


## 连续遥操作与 50Hz 状态流（2026-11-06）

按产品要求，将遥操作发送从逐条双往返改为一次握手后的持续输入，将高层任务和导航任务流从250ms检查改为目标50Hz周期快照。

- `Teleop::publish_velocity()` 同步提交当前新速度到有界发送队列，不等待ACK；后台单独读取窗口与回执。`latest_reply()`返回最近回执，`send_velocity()`保留单条应答等待，`hold/close`仍等待确认。
- 每个样本携带递增`sequence`及调用时捕获的机器人窗口。服务器每50ms签发可复用窗口，原始有效期固定350ms；窗口签发频率不限制速度发布频率。过期输入不被重新打时间戳，停止清除旧窗并在回执中提供新窗。速度积压可合并，控制消息保序。
- `Tasks.Watch`与`Navigation.Watch`每20ms交付当前状态快照，允许重复内容，保留真实源时间。绝对节拍计入读取耗时，限制迟到积压，终态/取消仍释放配额。
- `TeleopRequest/Reply.sequence=7`保留原字段编号。新Teleop时序要求SDK与Gateway配套更新；旧Gateway会在新SDK开流时被拒绝，不把字段可解析等同于二进制行为兼容。

当前修改通过Rust 8项单元、61项gRPC、2项消息格式和10项文档测试，以及Clippy；相关Gateway整合130项通过，包含新的25项遥操作协议及19项任务/导航专项。协议生成器2项通过。

本地连续遥操作测量使用真实Rust SDK、Python Gateway、NativeTeleopSession与LatestNativeTeleopPublisher，最后的native owner是测试替身。四组各5秒，全程仅连接loopback。回程延迟由独立TCP代理注入，未阻塞Gateway handler。

| 输入目标 | 注入回程延迟 | 本地提交 / 尝试 | fake native消费 | 实测消费频率 | 本地队列满 |
| --- | --- | --- | --- | --- | --- |
| 50Hz | 0ms | 250 / 250 | 250 | 49.98Hz | 0 |
| 50Hz | 80ms | 250 / 250 | 250 | 50.02Hz | 0 |
| 100Hz | 0ms | 396 / 500 | 396 | 79.22Hz | 104 |
| 100Hz | 80ms | 375 / 500 | 375 | 75.09Hz | 125 |

50Hz两组均完整送达、无观测到的拒绝，hold/close均确认；单次发布p99低于0.41ms。80ms代理目标在Windows实际回程延迟中位约92ms。100Hz压力测试未达到目标，本机约15.6ms定时粒度及补齐调度节拍产生发送突发，有界队列明确报告未入队输入，未通过扩大队列或重发旧输入掩盖结果。测量脚本、原始JSON和复跑说明位于`.local/evidence/teleop-50hz/`，入口为`examples/bench_teleop.rs`。

Tasks与Navigation另进行约6秒的四路并发本地fake-owner测量，两个gRPC流各301帧，平均分别50.005Hz和50.001Hz；直接读取约49.55/49.68Hz。Windows接收间隔p95约31.6ms、最大约46ms。结果证明本地平均约50Hz，不能作为每20ms硬截止或现场稳定性的证据。没有部署机器人或发送实机动作。


## 遥操作、任务流、定位状态与安全连接（2026-11-06）

本节保留上一阶段的实现和验证事实；当前频率与遥操作时序以本文首节为准。

| 入口与文件 | 本轮完成内容 |
| --- | --- |
| `Motion.Teleop`、`src/motion/teleop.rs`、LingTu `rpc/teleop.py` | 双向流接既有 native operator session，提供 `teleop/send_velocity/hold/close`，请求身份与回复逐项对应，速度输入使用机器人签发的一次性窗口 |
| `Motion.StandUp/SitDown`、`src/motion/posture.rs`、LingTu `services/posture.py` | `Command → Receipt` 复用 journal 与 motion guard，经 `NavCommands.stand/sit → nav DDS → driver → Brainstem` 提交姿态请求，SDK 超时采用至少 15秒 |
| `Tasks.Watch`、`src/tasks/watch.rs`、LingTu `rpc/task_run.py` | 当前 AgentPlanner run 每 250ms 检查，变化时发送，终态或不再保留时结束，关闭观察释放流名额 |
| `Run::state()`、`src/tasks/state.rs` | 单次查询与流共用状态解析，保留运行状态、失败原因与摘要 |
| `Localization.State`、`src/localization/state.rs`、LingTu `rpc/telemetry.py` | 将现有定位 owner 投影为类型化状态，保留源状态、地图、原因与可缺省就绪/新鲜度 |
| `src/client/mod.rs`、LingTu `rpc/security.py` 与 `rpc/server.py` | 所有 RPC 的 API key 鉴权、服务端 TLS、客户端系统信任根与私有 CA，错误凭据在业务处理前拒绝，无效 TLS 配置启动失败 |
| `Cargo.toml` | 为现有 Tonic 启用 `tls-ring`，修复仅启用系统信任根时 Rustls 缺少加密 provider 的 TLS panic |

遥操作每次速度输入包含两次请求应答往返，窗口有效期 350ms，连续 3 秒无输入时结束连接。速度回执确认 Gateway 可替换队列接纳，hold/release 的 `final_output_confirmed` 来自现有 native 最终输出屏障。遥操作实际频率和现场时延待测量。Tasks 变化流沿用当前 owner 的保留范围，进度、阶段和源采样时间缺失时保持未设置。

API key 与 TLS 分别配置，使用专用的 `LINGTU_GRPC_API_KEY`、`LINGTU_GRPC_REQUIRE_API_KEY`、`LINGTU_GRPC_TLS_CERT_FILE`、`LINGTU_GRPC_TLS_KEY_FILE`。未配置时采用开发用明文 listener。部署凭据与证书的实际配置仍需验收。

本轮定向验证已通过 28 项 Python 安全测试、13 项 Python 任务/定位测试、2 项 Rust TLS 集成测试、2 项 Rust 任务流/定位集成测试与 2 项 Rust 任务状态单测，对应 Python Ruff 通过。安全测试覆盖四种 RPC 形态的鉴权、重复凭据、TLS 私有 CA、错误主机名及无效证书配置。测试证书由本机 OpenSSL 临时生成并清理。

整合验证已通过 12 份 Gateway gRPC 回归文件的 180 项测试，其中包括当时 8 项姿态、15 项遥操作、28 项安全和 13 项任务/定位测试。RPC 全目录与对应服务/测试 Ruff 通过，协议生成器 2 项测试通过。Rust 全量通过 8 项单元、58 项 gRPC、2 项消息格式与 10 项文档编译测试，已包含新增 TLS、任务、遥操作和姿态使用片段。Rust 格式检查与 `cargo clippy --locked --all-targets -- -D warnings` 通过，站起/坐下 SDK 本地专项 1 项通过。最终原生验证与后续 Gateway 专项结果记录如下。

姿态回执描述接纳，后续动作状态通过身体状态查询或事件读取。BMS、电池、足端、光照、实际速度与里程计等来源缺口继续保留，综合健康与统一故障映射、单独控制权管理、断线恢复及实机端到端验收仍待推进。

WSL 中最终 `lingtu_driver` 与 `test_driver_io` 完整编译链接通过，8 项 CTest 通过，覆盖 DDS 到 mock Brainstem、过期与跨 boot 请求拒绝、重复请求 ACK、排队姿态取消和未知 ACK 保留。最终 `navd`、`lingtu_nav_client`、`test_command_ingress_controller` 与 `test_nav_client` 全部编译链接通过，两项专项 CTest 通过，包含真实 C ABI 站起/坐下到 DDS 再到 ACK 的往返。此前 ingress 独立编译执行及 `loop.cpp`、`dds/runtime.cpp`、`client.cpp`、`client_c.cpp` 完整文件的 WSL C++17 语法编译也通过。原生 Python adapters/module 专项 73 项、generated topic 检查及 Git diff 空白检查通过。

随后 Gateway 姿态专项增加原生原因映射，最终 12 项通过，Ruff 通过。`posture_ack_timeout`、`posture_cancel_requested`、`posture_action_ack_unknown` 与 `posture_publish_failed` 统一返回 `UNAVAILABLE` 和 `posture_ack_unknown`，保留未知提交事实。此前 180 项整合回归与此 12 项专项包含重叠测试，分别记录各自执行结果。

原生枚举冻结表有一项基线失败，`test_message_enum_generation.py::test_idl_and_python_preserve_wire_values` 的冻结表遗漏 HEAD 已有的 `InspectionTaskState.WAITING_NEXT_LOOP=14` 和 `SensorControlKind`。本轮保持该项失败可见。

硬件边界仍需验收。无 walking lease 的 Grounded/Transitioning 状态下，底层 stop 可能返回 `lease_missing`。接纳 ACK、姿态到位与物理停稳分别判断，本轮 native 测试使用 mock Brainstem。

## 逐文件完善（2026-11-06）

范围为 Rust SDK 手写文件、所属协议及对应机器人端接线。没有把接口声明当成机器人能力，没有部署或发送实机动作。

| 文件 | 本轮核对的函数/职责 | 处理结果 |
| --- | --- | --- |
| `src/client/mod.rs` | connect/with_config、请求与回执 | 保留共享连接、超时、身份校验；API key 元数据标为 sensitive，避免调试输出泄露 |
| `src/common/mod.rs` | new_request_id、text、Pose.validate | 检查请求身份和有限坐标，保留现有实现 |
| `src/error/mod.rs` | code、reason_code、details、rpc | 检查标准码、未知提交与原始错误详情，保留现有实现 |
| `src/system/mod.rs` | 身份、ping、状态、健康、能力、会话查询 | 既有原始查询保留；机器人未知状态不推断为健康 |
| `src/system/readiness.rs` | readiness_state/project | 检查缺失与false区别、错误字段，保留现有转换 |
| `src/system/status.rs` | robot_status | 新增类型化模式、就绪和已知软件急停；硬件急停/综合健康缺来源时保持未知 |
| `src/motion/stop.rs` | stop/emergency_stop/clear_emergency_stop | 检查共享owner、回执与未知提交，不把接纳当动作完成 |
| `src/motion/body.rs` | body_state/watch_body_state | 检查初始状态、事件、未知枚举与取消，保留源事件语义 |
| `src/motion/watch.rs` | watch_state/watch_state_at | 检查频率与取消，关闭观察不发停止命令 |
| `src/motion/status.rs` | status | 明确是native导航/控制状态投影，不是完整运动学数据 |
| `src/motion/control.rs` | control_status | 新增只读Brainstem控制状态，不申请租约、不推断SDK调用方拥有控制权 |
| `src/localization/mod.rs` | pose/watch_pose/status/relocalize | 源端坏坐标归Protocol；消息增加可缺省接收年龄和fresh，已知失效流返回UNAVAILABLE |
| `src/maps/mod.rs` | 列表、保存、操作查询/等待、加载/建图 | 核对身份、接纳与终态、超时不撤销远端操作，保留现有实现 |
| `src/maps/point_cloud.rs` | point_cloud | 核对元数据与PCD分块来源、取消/错误；下载示例验证总大小并清理本次不完整文件 |
| `src/maps/grid.rs` | grid | 核对25M格上限、地图身份、大小、值域与原点，保留现有校验 |
| `src/navigation/mod.rs` | 目标、查询、取消、路线、任务流/等待 | 保留现有合同；旧cancel允许缺省task_id，不强行改成新控制命令语义 |
| `src/navigation/control.rs` | pause/resume | 新增任务专属暂停/恢复，复用现有guard/journal/native ACK并校验回执身份 |
| `src/observations/camera.rs` | 快照/流、metadata | 流首帧等待订阅后发布；CameraInfo未知字段改为optional；单次快照仍允许缓存 |
| `src/observations/cloud.rs` | watch_point_cloud/decode | 核对PCLD长度/版本/flags/点数/坐标/RGB与reset，保留预览格式 |
| `src/observations/imu.rs`、`joints.rs` | 源事件流 | 保留可选加速度、源相对时间、单关节/整帧边界，不重采样 |
| `src/observations/motors.rs`、`voltages.rs` | 电机诊断/电压 | 保留真实源及错误；源缺有效位，0V可能是超时，未伪造电池数据 |
| `src/observations/scene.rs` | scene | 保留已有场景快照，强类型场景格式仍待明确 |
| `src/tasks/mod.rs` | 指令、get_run、cancel_run | 补取消回执run_id校验，冲突返回Protocol和未知提交，不重发命令 |
| `src/tasks/state.rs` | get_run_state/project | 保留任务owner终态、缺失当前任务与错误，不填造进度/时间 |
| `src/lib.rs` 与各域 `mod.rs` | 导出与模块注册 | 同步新增类型/方法，使用同一套Protobuf生成 |

验证：Rust 8项单元、47项gRPC合同、2项消息格式和6项文档示例通过；机器人端本轮相关13个测试文件共199项通过；两项协议生成/导入检查通过；受影响Python文件Ruff、Rust格式检查通过。

仍未完成：姿态命令完整仲裁链、连续遥操作及SDK控制权；BMS/足端/光照等真实数据源；综合健康/统一故障来源；鉴权和断线恢复；设备部署与实机验证。源端GRPC owner和ready_for_walk不能作为外部SDK调用方的控制授权。

## 运动状态目标频率调整（2026-11-05）

运动快照流默认目标最高频率调整为 100 Hz，`watch_state_at` 支持 1–100 Hz；快照处理耗时计入间隔。旧版机器人端仍可能只支持 1–4 Hz。该目标不改变数据源采样，实际频率、负载与实时性尚待机器人端验证；身体状态仍是变化事件，IMU 和关节仍由源事件驱动。

Windows 本机空设备夹具、单个 gRPC 回环订阅分别测得约 44 和 68 帧/秒，未达到 100 Hz；这不是机器人上的性能结果。

## 运动状态订阅频率（2026-11-04）

`motion().watch_state_at(frequency_hz)` 新增整数 1–4 Hz 目标最高更新频率；超范围返回 `INVALID_ARGUMENT`，旧 Gateway 不支持时返回 `UNIMPLEMENTED`。原 `watch_state()` 仍保持 4 Hz，两个入口共用 Gateway 状态读取和订阅名额。频率仅约束导航与控制状态快照读取及推送，不改变底层传感器采样；处理耗时、网络和源数据更新均影响实际表现。快照中的 `ts` 是 Gateway 评估时间，不是硬件采样时间。

Brainstem C++ 客户端已有 `subscribeImu`、`subscribeJoints`、`subscribeBodyState` 和最新值读取；IMU、关节流由设备上报驱动，目前没有硬件采样频率设置入口。DOSO 运动适配器尚未将这些数据接入 LingTu 观测链。Thunder V4 的 16 轴顺序已有来源，但适配器目前只有 `doso` 分类而无具体型号；各关节分次上报，Brainstem 时间为服务相对时间，不能直接冒充 LingTu 聚合快照的 epoch 时间或原始同步采样时间。需要确定型号与时间合同后再接入，不在控制循环内调用会等待首帧的 `startTelemetry()`。

## 运动状态持续订阅（2026-11-04）

`motion().watch_state()` 使用服务端流，沿用 `motion().status()` 的导航与控制状态快照。Gateway 当前每 250 ms 读取一次现有状态，运动和导航流共用 8 个订阅名额；关闭订阅只结束观察，不发送停止命令。SDK 与 Gateway 的调用、错误、取消及混合订阅资源限制已做本地验证；Rust SDK 到 Python Gateway 的互通使用底层服务测试替身。它不是 IMU、关节或电机硬件数据流，采样间隔不等于源数据频率。源码未部署，尚无仿真或真机验收。

运动域按功能整理为 `mod.rs`、`stop.rs`、`status.rs` 和 `watch.rs`，原有调用接口未改变。

日期：2026-11-02。版本：`doso-sdk 0.2.0-alpha.1`。

七域 Rust SDK 已由 REST/JSON 切换到 gRPC/Protobuf；LingTu Gateway 已实现对应的七个 service，并接入 Gateway 的启动和关闭流程。本轮完成本地实现、合同测试与跨语言验证，尚未完成仿真或实机验收。

## 本轮变化

- 删除 Rust 的 reqwest 传输、手写 JSON 回执解析、URL 编码与无独立职责的转发文件。`src/` 保留 `lib.rs` 与十个职责模块，共 11 个手写生产 Rust 源文件；七个领域各自集中相关操作。
- 连接、共享 Channel、metadata、超时和回执身份检查归入 [`client/mod.rs`](../src/client/mod.rs)。入口改为 `DosoClient::connect(endpoint).await`，七域方法改为异步调用，属于对旧预览 API 的不兼容升级。
- [`proto/`](../proto/) 统一维护一套协议。Rust 构建生成消息和客户端；[`generate_grpc.py`](../scripts/generate_grpc.py) 生成 LingTu Python 绑定。位姿、命令、导航任务、地图操作和产品操作使用显式字段；可扩展诊断保留 Protobuf `Struct`。
- LingTu 的 `gateway.rpc` 直接调用共享领域操作和现有 owner，没有通过旧 Gateway REST 接口转发 SDK 请求。Web 与 gRPC 共用导航提交/取消、停止、指令、健康、地图列表/保存/操作查询和重定位逻辑。
- 共享操作是从现有 HTTP 路由中提取并复用的业务实现，保留任务 journal、控制权、地图身份、原生 ACK 和操作状态投影；此次提取没有重写 SLAM、规划、运动仲裁或 ProductControl。
- 导航 `watch()` 提供服务端流，`wait(task_id, timeout)` 消费该流。Gateway 当前每 250 ms 读取已有任务状态；SDK 不再轮询导航任务。地图与产品事务仍按各自身份异步查询。
- 停止等待、超时或关闭状态流只结束订阅，不取消远端导航。取消和软件急停分别通过显式操作提交；写命令不自动重试，无法确认结果时保留请求身份和未知提交状态。
- mapd 不可达时保留 `UNAVAILABLE` 错误码；每个 Gateway 最多接纳 8 个导航状态订阅，超额返回 `RESOURCE_EXHAUSTED`，断开时释放名额，避免长订阅占满全部 16 个执行线程。

## 连接与启动

```text
Rust 应用 → DosoClient → gRPC / Protobuf :5052 → LingTu gateway.rpc
                                                    ↓
Web → Gateway HTTP :5050 ───────────────────→ 共享领域操作
                                                    ↓
                                   ProductControl / SLAM / mapd / navd
                                                    ↓
                                        既有运动链路 → Brainstem
```

`http://127.0.0.1:5052` 是 gRPC 的明文 HTTP/2 端点，载荷为 Protobuf。Web 的 HTTP `:5050` 和 ProductControl 的本机 HTTP `:5051` 仍保留；原生 DDS、mapd socket 与 Brainstem gRPC 也保持原有职责。

在 LingTu checkout 的运行环境执行 `python -m pip install -e ".[gateway]"`，随后沿用现有 ProductControl 启动流程。包含 Gateway 的 Host 会启动 gRPC listener；关闭 Gateway 时同时关闭 listener，无需单独部署一个 SDK 服务进程。生成绑定已包含在代码中，修改协议后才运行生成脚本。

客户端构建需要 Rust 1.94+ 和 `protoc`。安装、异步示例与方法列表见 [Rust SDK README](usage.md)。库通过 Cargo 本地依赖集成，尚未发布到 crates.io。

## 本地验证

| 范围 | 结果与证明边界 |
| --- | --- |
| Rust 格式与静态检查 | `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --locked --offline -- -D warnings` 通过 |
| Rust 合同测试 | `cargo test --workspace --locked --offline` 通过：5 项本地 Tonic gRPC 测试，2 项 README 示例编译测试 |
| Rust 包验证 | `cargo package -p doso-sdk --allow-dirty --locked --offline` 通过，包括解包后重新编译；产物 `target/package/doso-sdk-0.2.0-alpha.1.crate` |
| 只读示例 | `read_status --help` 成功且不建立连接 |
| LingTu gRPC | `test_grpc_sdk.py` 10 项、`test_grpc_lifecycle.py` 2 项通过，使用实际本地 gRPC listener 和生成 stub |
| 受影响 HTTP 回归 | 导航/命令/健康组 127 项通过，另 3 项因函数提取更新 monkeypatch 目标后通过；地图/定位组 151 项通过、2 项既有失败，见下文 |
| Python 静态检查 | 本轮 gRPC、共享操作、测试及生成脚本的 Ruff 检查通过；生成绑定不手工格式化 |
| 依赖锁文件 | LingTu `dev` 直接复用 `lingtu[gateway]`；`uv lock --offline` 与 `uv lock --check` 通过，只有依赖关系和 metadata 变更，没有包版本升级 |

Rust 测试覆盖类型化目标与接纳、回执/任务身份、Unavailable 时未知提交结果且不重试、导航状态流、本地等待超时不取消任务。README 中的连接和导航示例只验证可编译，没有执行其中的运动请求。

LingTu 测试覆盖 Ping、地图列表/保存/操作查询、带种子与全局重定位、NaN 拒绝、位姿不可用、导航提交/取消/租约拒绝、任务状态流、停止 ACK/拒绝，以及 listener 的启动、关闭与 deferred 模式。收尾新增两个地图入口的断连错误码测试，以及 8 个状态订阅期间 Ping/Stop 仍可用、第 9 个订阅拒绝、断开释放名额且不取消导航的测试。

### Rust → Python 跨语言验证

本地运行实际 Rust SDK、实际 Python Gateway gRPC handlers 和共享操作，得到：

```text
RUST_PYTHON_GRPC_INTEROP_OK: 7 domains, typed commands and navigation stream
OWNER_FIXTURE_CALLS 6
```

覆盖系统/就绪查询、目标提交与任务流终态、取消、停止、地图列表/保存/等待、带种子重定位、场景/相机状态和 Agent 指令提交/查询/取消。测试脚本及外部 Cargo consumer 位于本地忽略目录 `.local/evidence/grpc_interop.py` 与 `.local/evidence/grpc-consumer/`；仓库内长期保留的合同测试分别在 Rust 和 LingTu 的测试目录。

底层 goal、mapd、定位、Agent 和硬件 owner 使用测试替身。这证明跨语言协议与 Gateway 接线可用，不能证明原生规划、仿真运动或物理停稳。跨语言运行未覆盖 ProductControl 切换及切换后的重连。

### 两项既有失败

以下测试在加载本轮修改前的精确模块快照时同样失败，已确认属于基线问题，本轮未扩展修复：

- `tests/gateway/test_gateway_route_split.py::test_openapi_exposes_client_response_models`：WHEP OpenAPI 缺少 `application/sdp`。
- `tests/gateway/test_gateway_session_map_contract.py::test_operational_routes_validate_idle_json_contracts`：recording 的 JSONResponse 与响应模型不匹配。

## 后续边界

站起/坐下、连续遥操作及控制权交接、导航暂停/恢复、完整传感器订阅与统一巡检/跟随仍待接入，见 [V1 计划](v1-plan.md)。七域组织完成不代表每项目标能力已经实现。

当前 listener 使用明文 gRPC；客户端可以发送 API key metadata，但服务端尚未实施对应鉴权。部署访问边界和认证应在发布前完成验证。

本轮没有部署机器人、执行真机动作、提交或推送 Git、发布版本，也没有运行远程 CI。新增 CI 的 protoc 安装步骤仍待远程运行验证。未进行通信性能基准测试，不承诺延迟或吞吐提升比例。

## 仓库目录整理

Rust 单包已提升至仓库根目录，源码、协议、示例和测试分别位于 `src/`、`proto/`、`examples/` 和 `tests/`。C++ 底层客户端整体收进 `brainstem/`，根 CMake 入口继续供现有下游构建；Python HTTP 预览整体归档至 `archive/python/`，不再作为当前自动 CI 主线。

七份重复的领域 README 已合并到架构和使用说明；Python 与 Rust HTTP 的旧推进记录移至 `docs/history/`，保留原阶段事实。原 `build/` 中的源码快照、验证脚本和日志保存在本地忽略目录 `.local/evidence/`，没有作为缓存删除。目录调整没有增加机器人功能。

2026-11-03 在新目录完成以下验证：

- Rust 格式、Clippy、5 项 gRPC 测试和 2 项使用说明编译测试通过；Cargo 打包及解包编译通过，包内容不包含 `brainstem/`、`archive/` 或本地产物。
- 根 `proto/` 重新生成的三份 Python 绑定与 LingTu 现有绑定逐字节一致；本地 Rust → Python 七域互通复测通过，只读示例 `--help` 正常。
- Brainstem 在现成 WSL Ubuntu 中完成根 CMake 配置、子项目完整编译、2 项 CTest、安装及独立消费者运行；保留 `BrainstemClient 0.6` 安装合同。Windows 原生配置仍受本机缺少 Protobuf C++ 开发库限制。
- Python 归档完成 56 项测试与 51 项子测试、Ruff、wheel/sdist 构建及离线安装导入检查。未恢复其自动 CI。
- 45 个本地文档链接和 Git diff 空白检查通过。没有连接机器人、推送代码或执行远程 CI。

## 协议职责拆分（2026-11-03）

协议改为七个领域文件及 `common.proto`，每个领域维护自己的消息、枚举和 RPC，共用类型仅维护一处。智身官方固定版本资料的参考范围见 [架构说明](architecture.md#智身与宇树-sdk-参考)。Rust 和 Python 的生成入口已同步，LingTu 改用各领域绑定，并移除旧的总文件绑定。

拆分前后编译出的 24 个消息、4 个枚举、7 个服务描述逐项一致，保留 `doso.v1` 名称、字段号、枚举值、RPC 签名及流式属性；变化仅在协议文件和 Python 生成模块的归属。Rust 对外 API 无须改动。

验证通过：生成器重复生成、旧绑定清理及所有模块的包内导入检查；Rust 格式、Clippy、5 项 gRPC 测试、2 项示例编译和 Cargo 打包；LingTu 原有 12 项 gRPC 测试及 Ruff；七域 Rust → Python 本地互通。跨语言测试仍使用底层 owner 测试替身，没有新增机器人能力或实机验证。协议描述和本地验证日志保存在 `.local/proto-before/`。

## 返回合同首轮落地（2026-11-03）

按 [返回码与状态定义](results.md) 接通公共错误与领域结果：

- `Error::code()` 复用标准 gRPC `Code`。`reason_code()`、`details()` 从原始错误详情或完整 `Terminal` 派生；导航、地图、产品的失败终态不再只剩一段字符串。
- `proto/error.proto` 定义 `ErrorDetail`，经 `doso-error-bin` 传递业务原因和原始响应；Receipt、Task、MapOperation、ProductOperation 各追加一个 `reason_code` 字段。描述文件对比确认原字段、枚举和 RPC 没有改变。
- Gateway 用明确 `accepted` 投影兼容 `ok`，不会把已接纳但尚未完成的操作判成拒绝。接纳信息缺失时保留未知，互相矛盾时返回错误及原始证据。
- 查询失败或取消任务正常返回其状态；等待方法在非成功终态携带原始结果。合法 `UNKNOWN` 继续等待，未定义枚举值仍报协议错误。
- 实际复现了已执行写请求的超大回执被客户端以 `OutOfRange` 拒收；该情形现在保留请求身份与未知提交标记，没有自动重发。

验证结果：Rust Clippy、1 项单元测试、12 项 gRPC 测试、2 项文档编译和 Cargo 打包通过；Gateway 20 项 gRPC 测试与 Ruff 通过；协议生成器重复生成/导入检查通过。七域跨语言验证另覆盖控制权拒绝的稳定原因、未知原生 ACK、地图失败终态和操作身份，输出 `RESULT_CONTRACT_INTEROP_OK`。日志与描述文件位于 `.local/results-before/`。

本轮沿用上游现有错误分类，尚未统一全部业务场景的标准码归类；具体差异见返回合同。没有部署或连接真机，跨语言验证使用底层 owner 测试替身。

## System 观察式握手（2026-11-03，三项信息收敛前记录）

参照智身的“建立通信后完成应用握手”时序，该阶段的 `connect/with_config` 完成 `System.Handshake` 并确认协议版本后才返回客户端。握手发送协议版本、客户端标识和 SDK 版本，复用已有超时、metadata 和错误处理。连接采用观察行为。

该阶段的 `system().info()` 借用唯一的初次握手快照，返回配置机器人 ID、型号、声明环境及可取得的 LingTu 安装版本。型号读取与 Web bootstrap 共用 `robot_model()`，SN 无可靠来源时保持缺失。动态状态通过原查询读取，旧 Gateway 未提供 Handshake 时返回 `UNIMPLEMENTED`。

验证通过：Rust Clippy、1 项单元测试、17 项本地 gRPC 测试和 2 项文档编译；Gateway 26 项 gRPC 测试及 2 项 bootstrap 型号参数测试；协议生成器导入与重复生成检查；Cargo 打包与解包编译、只读示例 `--help`。跨语言验证新增协议版本、配置身份、声明环境、软件版本和缺失硬件 SN 的断言，输出 `SYSTEM_HANDSHAKE_INTEROP_OK`，同时保留此前七域和错误合同互通检查。日志位于 `.local/handshake/`。

一项既有 bootstrap 测试仍失败：`test_app_bootstrap_uses_run_plan_teleop_limits` 期望 `_teleop_max_speed=0.5`，当前为 `1.0`。在独立进程中恢复本轮提取 helper 前的原型号表达式后，同一断言仍失败；本轮未修改速度或配置行为。

此阶段完成观察式连接行为，没有新增控制权限会话、显式控制权申请/释放 API 或真机验收。配置身份尚不能作为硬件出厂唯一身份的证明，健康/就绪的内容验收继续单独推进。

## System 三项信息收敛（2026-11-03）

按产品要求，该阶段 `SystemInfo` 收敛为 `firmware_version`、`robot_version`、`robot_id`。固件版本统一读取运行发布包 `metadata.json.version`，机器人版本复用型号来源，机器人 ID 当时复用所选 RobotConfig。动态运行环境通过现有会话查询取得，握手兼容由客户端内部处理。最新字段来源与交付边界见[机器人信息与固件发布](release.md)。

本轮验证通过：Rust 格式与 Clippy、1 项单元测试、18 项本地 gRPC 测试和 2 项文档编译；Gateway 26 项 gRPC 测试与 Ruff；协议生成器重复生成/导入检查；六份文档的 42 个本地链接检查。Rust → Python 七域互通保留原结果合同检查，并验证从发布元数据读取的固件版本、机器人版本和机器人 ID。

SDK 与 Gateway 需要同步更新握手合同；旧合同明确拒绝连接。没有连接或部署真机，本地互通仍使用底层 owner 测试替身。当前发布包覆盖 LingTu 原生运行时，Brainstem 固件统一交付尚未接入。

## 永久 UUID 接线（2026-11-04）

Gateway 握手改为读取 `DOSO_DEVICE_ID`，移除 RobotConfig 身份读取与模板 ID 回退。缺失或空值保持 `None`；非空值要求规范小写、带连字符的 UUID v4，配置错误返回 `FAILED_PRECONDITION` 与 `invalid_robot_id`。SDK 不生成 ID，云端登记仍是分配入口。

`lt-host.service` 与现有 Agent 服务共享 `/etc/doso/robot-cloud/agent/agent.env`，Host 将它作为可选文件在 Product 会话环境之后加载。没有增加第二份设备配置，也不改变远程状态许可或运动控制流程。

验证通过：Gateway 30 项 gRPC 测试与 Ruff；Rust 1 项单元测试、18 项 gRPC 测试、2 项文档编译及格式检查；2 项现有部署合同测试。本地 WSL systemd 临时服务验证两端相同环境文件取得同一 UUID、持久配置覆盖会话占位值，以及 Host 缺少身份文件时仍可启动。Rust → Python 七域互通验证新 UUID 来源，日志位于 `.local/identity/`。

systemd 验证使用临时配置和只读断言进程，没有启动真实 Host/Agent 或连接机器人。未分配真实设备 ID、修改线上绑定或执行部署。App 的现有序列号认领入口改为机器人 ID，是下一步接线范围。

## App 认领与扫码入口（2026-11-04）

配套 App 源码已改为扫码取得永久 UUID，用户不手填长 ID；确认时沿用一次性绑定码。客户端提交、超时补查、取消后的确认与云端事务使用同一设备 ID。旧 App 的序列号请求仅转发到同一个 UUID 认领实现。资料页显示机器人 ID、原型号字段和原样固件版本；SDK 三项信息的 Agent 上报仍待接入。

App 已通过扫码/请求行为、完整类型、云端 handler 与真实本地 PostgreSQL 事务验证；原生相机权限配置及 Web 页面已检查。新版服务发布、含相机模块的手机安装包和真实扫码/设备认领尚未验收。当前产品规则与后续缺口由 App 的 `docs/production/DOSO_DEVICE_OWNERSHIP_AND_PAIRING.md` 统一维护。

## IMU 与关节观测（2026-11-05）

新增 `observations().watch_imu()` 和 `watch_joints()`。LingTu 对 real + DOSO RunPlan 注入只读 Brainstem 数据源，通过 Gateway gRPC 将源事件转给 Rust SDK；保留单关节事件或完整帧的边界、可选加速度及源相对时间，不重采样、不拼造同步帧。时间是 Brainstem 服务映射事件的相对时间，不是硬件采样时间；目前没有可信的采样新鲜度字段，也不能由 SDK 设置采样率。

Rust 格式、Clippy、1 项单元测试、24 项 gRPC 测试及 4 项文档测试通过。LingTu 的本地测试覆盖真实 RunPlan 装配、字段映射、远端 mTLS 配置、源错误、取消、共享订阅名额与 Gateway 停止。另用真实 Rust SDK、Gateway 和只读适配器连接本地 fake Brainstem，验证字段、缺失、错误、流结束及取消传播，脚本与结果位于 `.local/evidence/sensor-e2e/`。

此前直接连接真机 Brainstem 的短时复测接近 IMU 100 Hz 和每关节 100 Hz，但也出现过停流和抖动；这不等同于本次 SDK 端到端实机验收。本次没有部署或修改机器人，尚未核实新 Gateway 在机器狗上可访问 Brainstem；旧 Gateway 不支持新 RPC。当前仓库 DOSO 的默认远端目标为 `192.168.66.12:13145`，TLS 文件为空，而此前测量的是另一地址；需先核对实际 RunPlan 及 mTLS 配置、上游现有访问策略。持续稳定性、源时间准确性和通过 SDK 的实际频率仍须在现场测量。

## 电机诊断和身体状态（2026-11-05）

新增 `observations().motors()`、`motor_voltages()`，分别查询 Brainstem 电机状态及独立电压来源；新增 `motion().body_state()` 和 `watch_body_state()`，读取身体状态机当前状态或变化事件。Gateway 保留上游状态值与错误，流与已有订阅共享名额。接口和源数据限制见[观测能力](observations.md)及[使用说明](usage.md)。

本地验证使用真实 Rust SDK、Gateway、只读适配器和本地 fake Brainstem，覆盖状态和电压原值、源端错误及身体状态查询和事件。Gateway 和 SDK 各自增加合同测试。没有部署、调用机器人或验证实机状态转换。电机反馈无有效位；源端电压超时以 0 表示，不能判断其是否有效。

## 地图点云与相机图像（2026-11-05）

`maps().point_cloud(name)` 通过 Gateway 读取 mapd 的已保存 PCD，先返回地图身份和大小，再分块交付文件。`observations().camera_frame()` 和 `watch_camera()` 提供 JPEG 快照及变化帧，复用现有相机与媒体访问规则；它们不是 WebRTC 视频。订阅和地图下载与既有长流共享名额。可运行示例见[使用说明](usage.md)和[相机说明](camera.md)。

本地 Rust SDK → 真实 Gateway 联调使用模拟 mapd 文件和 JPEG 数据，验证 5 MiB 文件、身份、媒体凭据、帧变化与取消，并检查示例不覆盖已有文件。Rust、Gateway 合同测试及协议生成检查通过；这不证明现场 Linux mapd、设备相机或 WebRTC 可用。现场此前发现的 GS130W 相机尚未确认接入 LingTu 图传。本轮未部署或修改机器人；已知地址的 Gateway 与相机端口未能建立连接。

## 实时点云（2026-11-05）

`observations().watch_point_cloud(CloudKind::Scan / Map)` 订阅 LingTu 现有扫描或地图显示帧，`CloudFrame::decode()` 解码位置、可选颜色、坐标系、场景代次、序号、源时间与 reset。Gateway 的同步订阅复用 CloudViewer 的最新帧和发布器，与既有 WebSocket 共存；慢订阅者可能跳过旧帧，首帧可能来自缓存。点云经过现有显示采样和量化，源时间缺失时可能回退 Gateway 时间。

本地 Rust SDK → 实际 Gateway → CloudViewer 使用模拟点云验证缓存、更新、颜色、坐标、reset 和取消；Gateway 测试还覆盖跨线程发布、扫描订阅触发生产、超过 4 MiB 帧、共享名额和取消。App 地图页新增按钮，在浏览器打开 LingTu 的实时三维场景；当前没有 Rust SDK 到 React Native 的桥接。未部署 Gateway、构建安装 App 或验收实机点云。

## 外部运控模式（2026-11-07）

新增 `motion().controller()` 和 `motion().external_control()`，在同一个motion域提供Builtin/External控制器选择。客户自行选择策略语言、推理框架、模型格式与部署工具，SDK负责逐关节目标的接入及会话生命周期。公共消息集中于 `proto/messages/controller.proto`，Rust操作位于 `src/motion/external.rs`，调用顺序和16关节排列由[运动API](api/motion.md#自定义控制器)说明。

链路为Rust SDK双向gRPC → Gateway → native DDS/C ABI → lingtu-driver → Brainstem `RobotExternalControl`。driver持有后端私有token，所有中间层原样传递机器人签发的输入窗口。Gateway队列接纳、Brainstem帧验证与物理电机结果分别表达，`last_sequence`只表示通过机器人端验证的最新帧。补齐DDS读端、C ABI及Python的 `motor_disable_confirmed`，避免底层已有关闭确认而SDK恒读到false。

进入要求Grounded、电机关闭及无冲突控制任务；内置行为订阅和输出被暂停，传感器、CAN及保护保留。进入后先提交完整关节目标并确认 `last_sequence`，再显式使能。目标包含q/dq/kp/kd/tau，复用现有电机编码范围。控制流每20ms签发输入窗口，窗口有效100ms；SDK每100ms心跳维持300ms会话，使能输出另需100ms内的新目标。心跳不会保活旧目标。显式close关闭输出并恢复builtin idle，电机保持关闭；丢弃句柄、断流、半关闭与超时仅关闭输出并保留External隔离。过期且已确认关闭后，允许使用新请求ID重新接管。

本地验证结果：

- Rust：8项单元、79项gRPC、2项消息格式及11项文档测试，共100项通过；Clippy全target与格式检查通过。补充建流已接纳、但等待首个输入窗口失败时的请求身份回归。
- API文档：motion页面12段Rust示例以独立消费crate检查，12/12通过。
- 协议：重复生成、清理旧绑定、公开消息导出、Controller单次RPC与ExternalControl双向流属性、Brainstem包导入，2项生成测试通过；SDK与Brainstem的external协议副本一致。
- Gateway：外部控制28项合同测试通过，Ruff通过；覆盖原生ACK身份、原窗口、目标数量/字段、共享配额、取消和清理。
- C++ SDK：ExternalControl本地gRPC测试1/1通过，既有Client/Profile重新构建后2/2通过，库已在本地安装给driver构建使用。
- LingTu：adapter/commands共84项通过；native client真实DDS/C ABI与command ingress测试2/2、driver posture/profile测试2/2通过。补齐关闭确认后，external_bridge再次1/1、Python外控6项通过，生成检查及Ruff通过。
- Brainstem：thunder全量350项、thunder_brain全量84项通过，共享frequency_watch 8项、skinny_dog_algebra 10项通过。最后低电压锁存接线修改后，34项外控runtime、本机gRPC、真实关节fake映射、硬件启动与关机相关回归通过；thunder/thunder_brain全包严格分析零issue。

Gateway最终组合回归曾为119项通过、1项旧遥操作计时用例失败：该用例在120ms输入窗口内完成握手、首次往返及40ms等待后，第二条输入已被判过期。随后仅复跑该用例1/1通过，外控28项与该用例组合29/29通过；目标用例开始时仅有主线程，无外控请求线程残留。组合复跑中同窗口输入仍余73ms、旧窗口输入已过期36ms，处理符合原截止时间。本轮未还原首次失败的具体调度耗时，保留其短窗口计时敏感的记录，未修改旧遥操作生产语义或测试窗口。

以上是本地合同、实际本机传输和测试替身验证。本轮未部署、未启动真机入口、未扫描CAN或使能真实电机。持续送帧频率、无线时延、目标机保护和电机关闭时序仍待硬件验收；编码范围校验不等同于机械关节限位，`motor_disable_confirmed`不证明物理静止。
