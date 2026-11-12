# DOSO-SDK cloud architecture image edit

Edited with the built-in image tool. The tool does not expose model-version selection.

Reference: doso-sdk-architecture-20261107.png (the corrected prior version).

Cloud facts checked against D:/DOSO/robot-cloud/ARCHITECTURE.md, CONTRACT.md and PROGRESS.md. The real-device cloud link is not claimed deployed or accepted. No cloud forwarding of external joint commands is implemented.

Use case: technical architecture diagram edit.
Edit the supplied DOSO Rust SDK architecture image. It already has a well-checked seven-domain SDK and robot control diagram. Preserve those exact domain labels, component names, source-code strip, technical typography, restrained navy/teal colors, and particularly the two separate lifecycle/stream routes and parallel Builtin/External motor outputs.

USER CHANGES:
A. Remove the large top headline "DOSO Rust SDK 架构" completely. Do NOT add another large overall title. The small descriptive subtitle may be removed to free space. No empty headline-sized blank band.
B. Add the real existing cloud management and remote-state architecture. Expand the canvas vertically to make room. Do not shrink the existing text to illegibility. This is a revised architectural overview, not merely a conceptual cloud icon.

COMPOSITION:
Create a clear full-width horizontal CLOUD panel above the current two-column client/robot layout. No giant global headline. CLOUD panel has a modest section label "云端管理与远程状态" using the same scale as other group headers. Retain the SDK on the left and robot on the right below it. Add a distinct cloud-link process in the robot enclosure ABOVE the Host/Gateway. Preserve an uncluttered layout with generous routing corridors and orthogonal arrows.

CLOUD panel components and exact labels:
1. "账号 / 设备后端"
   subtitle "设备绑定 · 用户许可"
2. "PostgreSQL"
   subtitle "设备归属 · 最新状态"
3. "doso-cloud-gateway"
   subtitle "远程状态查询与同步"
4. "Mosquitto"
   subtitle "MQTT Broker"

Cloud component connections MUST be:
- The existing caller box "客户程序 / App / 自定义策略" has an upward arrow to "账号 / 设备后端", labeled "App · HTTPS". This is a separate App management route; keep the existing caller -> Rust SDK route.
- "账号 / 设备后端" <-> "PostgreSQL" via a short database branch.
- "账号 / 设备后端" <-> "doso-cloud-gateway", labeled "HTTPS".
- "doso-cloud-gateway" <-> "Mosquitto", labeled "MQTT".
Do NOT insert the Rust SDK into the cloud server group. Do NOT draw a cloud-to-SDK gRPC control proxy.

ROBOT cloud entry:
Inside the robot enclosure add ONE distinct box "doso-cloud-link" with subtitle "机端云连接 · 待真机接入".
This cloud link is a separate process from the existing Host/Gateway.
Connect "Mosquitto" <-> "doso-cloud-link" with a dashed violet connection labeled "MQTT 5 / mTLS".
Connect "doso-cloud-link" <-> "Brainstem" with a separate dashed violet route labeled "本机只读 gRPC · 状态".
Route this read-only connection along the robot group's outer edge so it does not cross the native-control paths. It must not lead to CAN motors or the driver command port.
Small cloud note: "远程状态链路：真机接入待验收". Dashed lines represent the uncompleted real-device integration.
Add one short shared identity note near cloud: "统一 robot_id：UUID v4".

Preserve and correctly show LOCAL CONTROL:
- All seven SDK methods exactly once: system(), motion(), localization(), maps(), navigation(), observations(), tasks(), with their existing descriptions.
- DosoClient shared connection.
- Local public "gRPC / Protobuf :5052" connection directly to robot "Host / Gateway".
- Inside Host keep "native client（本地库）".
- Lifecycle "进入 / 使能 / 禁用 / 交还": Host/native client -> navd control arbitration -> lingtu-driver.
- Stream "关节帧 + 心跳 · DDS": native client -> lingtu-driver directly, bypassing navd.
- driver -> Host return labeled "控制器状态 · DDS".
- driver <-> Brainstem via internal gRPC.
- In Brainstem, "Builtin / 内置运控 · Profile" and "External / 客户关节目标 / q / dq / kp / kd / tau" are two alternate PARALLEL sources, independently merge into ONE RealJoint then ONE CAN 电机. Keep "控制器选择：二选一".
- Existing secondary owners ProductControl, SLAM / 定位, mapd, 任务 owner, 设备 / 感知服务 stay present in the robot group.
- Customer model inference stays outside SDK.

Bottom retain source strip:
"API 说明 / docs/api/"
"RPC 定义 / proto/*.proto"
"消息格式 / proto/messages/"
"Rust 实现 / src/"
"外控入口：src/motion/external.rs"

Footer concise exact facts:
"云端：设备管理与远程状态；SDK：本地接入与控制。"
"代码架构 · 端到端真机验收待完成"

Do not invent cloud joint control, cloud model deployment, video production readiness, OTA or any new cloud APIs. Do not add unverified real-time claims. Omit media/video from this image to keep the illustrated current scope precise. All Chinese must be crisp and correctly spelled. Preserve original main content, expand/reflow only to accommodate cloud and new robot cloud-link.

## Final connector cleanup

Make one tiny cleanup to this technical diagram. Preserve every component, all text, all positions, all dashed purple cloud status links, all navy local-control links, the green labeled native-client-to-driver DDS link, and the removed-title layout exactly.
Remove ONLY the unintended thin, unlabeled vertical GREEN line at approximately x=91% of image width, from y=31.5% to y=67.2%, running from the bottom of the "doso-cloud-link" box down to the top edge of "Brainstem". Restore the local background where that one green line was. The cloud-link must have only its MQTT dashed purple link and its already-labeled dashed purple "本机只读 gRPC · 状态" connection to Brainstem. Do not change either purple link. Do not remove the intended green L-shaped "关节帧 + 心跳 · DDS" route from native client to lingtu-driver at x about87%.
Do not add any replacement line. No layout or typography changes. Keep the rest pixel-consistent.
