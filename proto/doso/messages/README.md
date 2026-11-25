# DOSO message formats

This directory is the single source for SDK message and enum definitions.
The files in `proto/` declare RPCs and publicly import these messages so
existing service and generated type names remain available.

| File | Contents |
| --- | --- |
| `common.proto`, `error.proto` | Shared pose, time, command, receipt and errors |
| `system.proto` | Identity, robot status, battery, faults and control ownership |
| `motion.proto` | Body state, velocity and odometry |
| `controller.proto` | Builtin/external controller state, joint targets and external control stream actions; publicly exported by `proto/motion.proto` |
| `localization.proto` | Pose and localization status |
| `maps.proto` | Map records, operations, point-cloud content and occupancy grid |
| `navigation.proto` | Goals, tasks and paths |
| `observations.proto` | IMU, joints, motors, camera, point cloud, contact and light |
| `tasks.proto` | Instructions and high-level task state |

New message formats do not by themselves add RPCs or connect hardware data.
See [the message catalog](../../docs/message-catalog.md) for fields, current
RPCs, data sources and validation status.
