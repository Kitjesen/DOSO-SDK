# DOSO-SDK architecture image prompt

Generated with the built-in image tool. The tool does not expose model-version selection.

Use case: infographic-diagram.
Create ONE polished, accurate Chinese technical architecture diagram for the existing DOSO Rust SDK repository. Landscape 16:9, high resolution, crisp readable Chinese typography, clean white background, restrained navy and teal engineering colors, precise grid, generous spacing, thin orthogonal arrows and clear group boundaries. Flat diagram, no photorealism, no decorative robot render, no gradients, no visual clutter. Chinese labels must be clear and correctly spelled; preserve exact English identifiers. Prefer large legible labels over excessive small print.

Title exactly: "DOSO Rust SDK 架构"
Subtitle exactly: "七域接口 · gRPC / Protobuf · 内置与外部运控"

LAYOUT: three clearly bounded vertical regions, with a source-code index strip across the bottom. Left region about 33% width, a narrow shared transport bridge in middle, robot-side region about 56%. Keep every arrow direction unambiguous and all boxes separate. No overlapping text or crossed connector bundles.

LEFT — calling side:
At top, OUTSIDE the SDK boundary, one box: "客户程序 / App / 自定义策略" and a smaller line "观测处理 → 模型推理 → 关节目标". This is customer-owned code; model inference is NOT inside the SDK.
Below it a clearly outlined group titled "DOSO Rust SDK · 调用方运行".
Top box inside: "DosoClient" and "连接 · 握手 · 认证 · 共享通道".
Below DosoClient arrange seven equally weighted domain cards in a compact two-column grid, last card spanning width. Text exactly:
"system()" / "身份 · 能力 · 健康 · 就绪"
"motion()" / "内置操控 · 外部关节控制"
"localization()" / "位姿 · 定位 · 重定位"
"maps()" / "建图 · 保存 · 加载 · 地图内容"
"navigation()" / "目标 · 路线 · 任务状态"
"observations()" / "IMU · 关节 · 相机 · 点云"
"tasks()" / "高层指令 · 任务运行"
A shared connection from DosoClient fans into these domain cards; these are one SDK, NOT seven independent services or processes. Use a single shared transport connection to robot public server. Add a small source label "src/<领域>/".

MIDDLE — shared public connection:
A clean single bidirectional bridge from SDK group to the robot's Host public server, labeled "gRPC / Protobuf" and ":5052". Smaller label "请求 · 回执 · 状态流". Do NOT label HTTP/JSON.

RIGHT — robot side, group titled "机器人端 · LingTu + Brainstem":
At top, a wide box "Host / Gateway" with subtitle "七域 gRPC 服务 · 请求路由 · 状态反馈".
Within this Host area include a small attached box "native client（本地库）", not a separate daemon.
Below server, a shallow row of owner cards connected through a branching bus labeled "现有领域执行方":
"ProductControl" / "产品生命周期"
"SLAM / 定位" / "位姿与定位"
"mapd" / "地图与保存产物"
"任务 owner" / "高层任务"
"设备 / 感知服务" / "观测数据"
These are supporting domain owners, not components inside the Rust SDK. They can be small secondary boxes, but remain readable.

MAIN DETAIL INSIDE ROBOT REGION — a larger focused panel titled "motion · 控制接管与电机执行":
Draw two explicit routes from the Host native client toward one shared driver:
1. A thin navy line labeled "进入 / 使能 / 禁用 / 交还" goes THROUGH a box "navd · 控制仲裁" then reaches "lingtu-driver".
2. A teal parallel route labeled "关节帧 + 心跳 · DDS" goes DIRECTLY from Host's native client to "lingtu-driver", bypassing navd. Do NOT route these joint frames through navigation planning or any velocity converter.
Inside or just beneath driver, small text "DOSO 适配 · 命令与状态".
Then driver connects with a bidirectional link labeled "内部 gRPC" to a substantial "Brainstem" group.
Brainstem contains two side-by-side alternative controller boxes: "Builtin" / "内置运控 · Profile" and "External" / "客户关节目标". Add one concise caption "控制器选择：二选一". Both controller choices merge into ONE output box "RealJoint" and then ONE final box "CAN 电机". No two competing simultaneous actuator outputs. The External targets are explicitly labeled "q / dq / kp / kd / tau". Do NOT show a q/dq -> chassis velocity conversion. The motor electrical control loop stays at the actuator.
Add a modest return arrow from driver to Host labeled "控制器状态 · DDS". Add another subtle feedback arrow returning toward caller/observations labeled "IMU / 关节反馈", making clear observations continue during External mode.

BOTTOM SOURCE INDEX STRIP — four simple aligned cells, this strip is documentation, NOT runtime nodes:
"API 说明" / "docs/api/"
"RPC 定义" / "proto/*.proto"
"消息格式" / "proto/messages/"
"Rust 实现" / "src/"
Add a short line below: "外控入口：src/motion/external.rs"
Footer, visually secondary but readable:
"SDK 封装接入；客户程序运行模型；机器人端执行与保护。"
"代码架构 · 真机验收待完成"

ACCURACY CONSTRAINTS:
- All seven domain identifiers must appear exactly once in the SDK domain grid.
- Shared DosoClient channel and server.
- Customer inference outside SDK; do not invent model upload, cloud deployment, ROS, or policy management services.
- Lifecycle via navd; streaming joint frames and heartbeat directly via DDS to driver.
- Builtin and External share hardware output and are mutually exclusive controller choices; Profile belongs to Builtin.
- Keep the architecture clean and practical, not a dense code listing. Do not include unverified frequency or real-time performance claims.
- No model-version claim, no watermark, no invented logos.

## Connector correction pass

Use case: precise-object-edit / technical architecture diagram correction.
Edit the attached DOSO Rust SDK architecture diagram. Preserve the title, all seven SDK domain cards and their labels, all existing architecture component names, overall layout, source index strip, clean engineering style and high Chinese text legibility. Change only the connector topology needed for technical accuracy, with small layout adjustments inside Brainstem allowed.

Required precise corrections:
1. Shared public gRPC: the ":5052" bidirectional connection must visibly terminate at the actual "Host / Gateway" box, using a routed orthogonal line through the empty gap. It must not terminate only at the robot outer boundary or navd.
2. Lifecycle path: ONE navy line goes from "native client（本地库）" to "navd · 控制仲裁" and then from navd to "lingtu-driver". Label that lifecycle path "进入 / 使能 / 禁用 / 交还". Use arrowheads toward navd and then driver. No disconnected lifecycle origin.
3. Joint data path: ONE green line runs directly from native client to lingtu-driver and bypasses navd. Label it "关节帧 + 心跳 · DDS". Remove duplicate equivalent green/navy downward data paths. Arrowhead at driver.
4. Status return: ONE distinct navy return line runs FROM lingtu-driver TO native client / Host, label "控制器状态 · DDS". Arrowhead at Host, not at driver. Avoid crossing the stream line.
5. In Brainstem, Builtin and External are ALTERNATIVE parallel controller outputs. Remove the direct line from Builtin into External. Give Builtin its own output connector, give External a separate output connector, merge the two at one clear junction before ONE RealJoint box. Then ONE arrow from RealJoint to ONE CAN 电机 box. Keep "控制器选择：二选一". Neither controller may route through the other. Preserve "Builtin / 内置运控 · Profile" and "External / 客户关节目标 / q / dq / kp / kd / tau". You may rearrange these four boxes within Brainstem into two top boxes and a common lower output row if that improves parallel-merge clarity, without obscuring text.
6. Existing "IMU / 关节反馈" return arrow must have a connected origin at "设备 / 感知服务" and an arrowhead at SDK observations(). Route it around content; no free-floating line. It represents observational feedback, distinct from motor targets.

Maintain readable Chinese, exact identifiers, all seven domains exactly once, model inference outside SDK. No new services, no added performance claims, no new decorative elements. Deliver the corrected complete high-resolution landscape diagram.

## Final correction pass

Edit this architecture image with ONLY the three following connector corrections. Preserve every title, all text, every box and position, all seven SDK cards, and especially preserve the now-correct parallel Builtin/External merge into RealJoint. Do not redesign.
1. DELETE the entire long green return connector labeled "IMU / 关节反馈", including that label and the green line leading left to observations(). The diagram already has a shared bidirectional gRPC channel, so no extra sensor feedback connector is needed. Keep the observations() card intact.
2. The ONLY remaining green connector must start at the small "native client（本地库）" box within Host / Gateway. Route it down the empty far-right space, then LEFT toward the top of lingtu-driver with an arrowhead entering lingtu-driver. Label that exact line "关节帧 + 心跳 · DDS". Disconnect this line completely from the "设备 / 感知服务" owner box: that box must retain only its existing upper owner-bus connector. There must be NO arrow from the device/perception box into driver. One native client -> driver green line only.
3. The navy lifecycle arrow entering navd currently starts disconnected inside motion. Connect its START to the Host / Gateway box using an orthogonal route along the EMPTY LEFT INNER MARGIN of the robot-side frame, outside the owner cards. End at navd and retain navd -> driver. Label it "进入 / 使能 / 禁用 / 交还". No floating unconnected start. Do not pass through any owner cards.

Keep the navy driver -> Host return line labeled "控制器状态 · DDS" and the corrected public :5052 -> Host connection unchanged. Keep all Brainstem internal wiring unchanged. Keep crisp Chinese text and same overall image. Remove confusing duplicate lines.
