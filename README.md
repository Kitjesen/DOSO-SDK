# DOSO SDK

The official thin C++17 client SDK created by **DOSO** for external programs
that control a Brainstem Server. The public headers expose DOSO's stable
`brainstem::Client` API without leaking gRPC, Protobuf, lease tokens, or
generated protocol classes.

```text
LingTu or another C++ application
                |
                v
       brainstem::Client
                |
                v
private gRPC/Protobuf implementation
                |
                v
       Brainstem Server
```

This repository contains the C++ autonomous-motion and robot-telemetry client.
It intentionally exposes domain types rather than every raw Brainstem RPC.

## Supported API

```cpp
#include <brainstem/client.hpp>

brainstem::Config config;
config.target = "192.168.123.18:13145";
config.client_id = "lingtu-driver@robot";
config.tls.ca_file = "/etc/brainstem/tls/ca.crt";
config.tls.certificate_file = "/etc/brainstem/tls/lingtu-driver.crt";
config.tls.private_key_file = "/etc/brainstem/tls/lingtu-driver.key";

brainstem::Client client(config);
brainstem::Result ready = client.refresh();
brainstem::Result moving = client.move({0.2, 0.0, 0.3});
brainstem::Result stopped = client.stop();
```

The public surface deliberately stays small:

- `connect()` checks reachability and reads one control-state snapshot.
- `refresh()` acquires or renews the explicit control lease. Initial acquisition
  also sends and confirms a checked zero command.
- `move()` sends body-frame physical velocity: `x/y` in m/s and yaw in rad/s.
  It never converts values to `[-1, 1]` or silently clamps them.
- `stop()` sends a checked zero velocity and releases the lease. Only
  `result.confirmsStop()` means both operations were confirmed.
- `standUp()` and `sitDown()` stop and release locomotion first if this client
  owns the lease, then submit the posture action.
- `enableMotors()`, `disableMotors()`, `setJointZero()`, and
  `clearMotorFaults()` expose authenticated hardware maintenance operations.
- `getServerStartTime()`, `getBodyState()`, `getMotorStatus()`, and
  `getVoltage()` perform typed request/response reads.
- `startTelemetry()`, `telemetry()`, and `stopTelemetry()` manage selected
  background streams and expose one thread-safe latest-value snapshot.
- `latestImu()`, `latestJoints()`, and `latestBodyState()` copy one selected
  cache directly; none of these getters perform network I/O.
- `subscribeBodyState()`, `subscribeImu()`, and `subscribeJoints()` open explicit,
  cancellable raw streams for callers that need every frame.
- `state()` returns the most recently decoded state without network I/O.

`move()` enforces the Server's 20 ms minimum checked-command interval. A lost
ACK is retried once with the same sequence and payload, matching the Server's
idempotency contract. Preemption, a rejected sequence, or a failed lease renewal
invalidates local motion readiness.

## Managed telemetry

Select only the streams the application needs. Starting telemetry waits up to
`Config::timeout` for an initial usable sample from each selected stream. Reading
`telemetry()` only copies the local cache; it performs no network I/O and can run
alongside the serialized motion-control loop.

```cpp
brainstem::TelemetryOptions options;
options.imu = true;
options.joints = true;
options.body_state = false;

brainstem::Result started = client.startTelemetry(options);
if (started.ok) {
  brainstem::TelemetrySnapshot snapshot = client.telemetry();
  if (snapshot.imu.available && snapshot.imu.fresh) {
    const auto angular_velocity = snapshot.imu.value.angular_velocity_rps;
    const auto linear_acceleration = snapshot.imu.value.linear_acceleration_mps2;
  }
  if (snapshot.joints.complete() && snapshot.joints.fresh()) {
    const brainstem::JointState &front_right_hip = snapshot.joints.joints[0];
  }
}

client.stopTelemetry();
```

`startTelemetry()` defaults to IMU, joints, and body state. Calling it again
applies a new selection: disabled streams are cancelled and inactive selected
streams are restarted. `active` reports whether the Server stream is still running;
`available` and the joint `valid_mask` report whether cached data may be read.
IMU `age`/`fresh` and the joint `fresh_mask` use
`Config::telemetry_stale_after`, which defaults to 500 ms.

The joint cache has 16 stable slots indexed by joint ID. A full Brainstem frame
populates all slots; later single-joint messages update only their own slot.
`elapsed[id]` preserves the matching Brainstem sample timestamp. Call
`complete()` before assuming all 16 positions are known.

Motor temperature, per-motor voltage, faults, and aggregate voltage are
request/response data in `brainstem_api` 2.2, not streams. Read them when needed
with `getMotorStatus()` and `getVoltage()`; they do not require
`startTelemetry()` or a locomotion lease.

The IMU cache contains body-frame angular velocity in rad/s, an orientation
quaternion in `w/x/y/z` order, and session-relative elapsed time. With a 2.2
Server it also contains body-frame linear acceleration in m/s². The acceleration
is an `std::optional`: it is empty when connected to a compatible older Server
that does not send field 4.

## Telemetry reference

The SDK currently returns these public types:

| API | Information |
| --- | --- |
| `connect()` / `state()` | Connectivity, readiness, FSM, motor enable/fault, lease owner and remaining time, command sequence, accepted/rejected counts. |
| `getServerStartTime()` | Brainstem process start time as a UTC `system_clock::time_point`; use it to detect a Server restart and invalidate session-relative assumptions. |
| `getBodyState()` / managed or raw body-state stream | Grounded, standing, walking, transitioning, and active posture/gesture transition. |
| `getMotorStatus()` | Per-motor online flag, status code, temperature, voltage, position, velocity, torque, and error codes. |
| `getVoltage()` | Voltage values in Brainstem protocol order. |
| managed or raw IMU stream | Body-frame angular velocity in rad/s, optional linear acceleration in m/s², orientation quaternion, and session-relative timestamp. |
| managed or raw joint stream | Position, velocity, torque, status, joint ID, and session-relative timestamp; full snapshots and single-joint updates are merged by the managed cache. |

For advanced per-frame processing, each raw subscription owns one worker thread
and one Server stream. The callback runs on that worker thread. Cancellation and
completion are explicit:

```cpp
auto imu = client.subscribeImu([](const brainstem::ImuSample &sample) {
  // Keep this callback short; hand off the sample to your own queue if needed.
});

imu->cancel();
brainstem::Result finished = imu->wait();
```

Destroying a subscription also cancels and joins it. Read-only telemetry does
not acquire the locomotion lease.

## Result model

Every network operation returns `brainstem::Result`:

| Field | Meaning |
| --- | --- |
| `ok` | The requested operation completed successfully. |
| `transport_ok` | The RPC reached the Server and produced an application-level response. |
| `accepted` | The Server accepted the request; it does not imply a physical posture transition completed. |
| `stop_confirmed` | Checked zero and lease release were both confirmed. |
| `state` | Latest unified control state. |
| `error` | Stable reason details for logging and diagnostics. |

Use `confirmsStop()` for safety-sensitive stop decisions rather than checking
only `ok`.

## Control lease and identity

The motion client is serialized: call motion methods from one control loop, or
serialize them in the application. There is no hidden lease heartbeat. A lease
is a short-lived exclusive token proving that this client owns the locomotion
path; it prevents navigation, a test tool, and a remote controller from sending
competing velocity commands. Call `refresh()` before
`state.lease_remaining_ms` reaches zero; accepted `move()` calls also renew it.
The Brainstem 2.2 default is 300 ms. Lease expiry or remote-control preemption
zeros the Brainstem motion path and makes the client not ready. `stop()` first
sends a checked zero command and then releases the lease.

`Config::client_id` is required. On hardware it must exactly match the
authenticated certificate principal configured by the Brainstem Server, using
the `name@scope` form such as `lingtu-driver@robot`.

Supplying any TLS file requires the CA, client certificate, and private key.
`tls.server_name` is an optional certificate-name override. An empty TLS
configuration is rejected unless the caller explicitly sets
`config.allow_insecure = true`; that opt-in is only for loopback development
and isolated tests. Production remote motion uses mutual TLS.

## Relationship to Unitree SDK2

| Unitree SDK2 | DOSO SDK |
| --- | --- |
| `SportClient::Init()` | construct `Client`, then `connect()` |
| internal control ownership | `refresh()` manages an explicit lease |
| `Move(vx, vy, wz)` | `move({vx_mps, vy_mps, yaw_rps})` |
| `StopMove()` | confirmed checked-zero plus lease release |
| `StandUp()` | `standUp()` |
| `Sit()` | `sitDown()` |
| typed state subscriptions | body-state, IMU, and joint subscriptions |
| SDK error code | `Result` and `ControlState` |

## Build, test, and install

Ubuntu dependencies:

```bash
sudo apt-get update
sudo apt-get install -y build-essential cmake pkg-config protobuf-compiler \
  protobuf-compiler-grpc libprotobuf-dev libgrpc++-dev
```

CMake accepts either the upstream gRPC package configuration or Debian/Ubuntu's
`grpc++.pc` pkg-config metadata.

Build a clean checkout:

```bash
bash scripts/verify.sh
```

That command configures, builds, runs the SDK tests, installs into a temporary
prefix, and builds a separate package consumer. For a persistent local install:

```bash
cmake -S . -B build -DBUILD_TESTING=ON -DBRAINSTEM_BUILD_EXAMPLES=ON
cmake --build build --parallel
ctest --test-dir build --output-on-failure
cmake --install build --prefix "$PWD/install"
cmake -S tests/package_consumer -B build/consumer \
  -DCMAKE_PREFIX_PATH="$PWD/install"
cmake --build build/consumer --parallel
./build/consumer/brainstem_client_consumer
```

Applications consume the installed package with:

```cmake
find_package(BrainstemClient CONFIG REQUIRED)
target_link_libraries(my_robot PRIVATE brainstem::client)
```

The repository vendors the exact `brainstem_api` 2.2 input files used for code
generation under `protocol/`; consumers do not need the Brainstem monorepo or a
protocol submodule.

## Motion example

The example does nothing unless target, identity, physical velocity, and a
positive duration are supplied. It renews the lease while motion is active and
always requests a confirmed stop at the end.

```bash
./build/brainstem_motion_example \
  192.168.123.18:13145 lingtu-driver@robot \
  0.20 0.00 0.00 1000 \
  /etc/brainstem/tls/ca.crt \
  /etc/brainstem/tls/lingtu-driver.crt \
  /etc/brainstem/tls/lingtu-driver.key
```

Omit the final three TLS paths only for an explicitly insecure local Server;
the example then enables the SDK's explicit insecure-development opt-in.

## Telemetry example

The telemetry example opens only read-only streams. It prints changed IMU,
joint-cache, and body-state snapshots for the requested duration and never
acquires the locomotion lease:

```bash
./build/brainstem_telemetry_example \
  192.168.123.18:13145 observer@robot 5000 \
  /etc/brainstem/tls/ca.crt \
  /etc/brainstem/tls/observer.crt \
  /etc/brainstem/tls/observer.key
```

## Maintenance example

Maintenance commands change robot hardware state and therefore require an
authenticated actuator principal. `clear-faults` with no IDs clears all faults
and the current Server disables the motors; `clear-faults:2,15` targets only the
listed joints.

```bash
./build/brainstem_maintenance_example \
  192.168.123.18:13145 maintenance@robot clear-faults:2,15 \
  /etc/brainstem/tls/ca.crt \
  /etc/brainstem/tls/maintenance.crt \
  /etc/brainstem/tls/maintenance.key
```

Available commands are `enable`, `disable`, `set-zero`, `clear-faults`,
`clear-faults:<ids>`, `stand-up`, and `sit-down`. Joint zero remains guarded by
the Server and succeeds only while the body is grounded.

## Current limits

- Brainstem inference history, robot parameters, and profile management do not
  yet have public DOSO SDK facades.
- Camera, LiDAR, odometry, and maps are not present in `brainstem_api` 2.2 and
  therefore do not come from this SDK.
- Managed streams do not reconnect silently. If `active` becomes false, call
  `startTelemetry()` again with the desired selection; cached freshness remains
  explicit.
- Gesture and speed-mode RPC names exist in the protocol, but Brainstem Server
  currently returns `UNIMPLEMENTED`; they are not advertised as SDK features.
- No recover-stand or damping action because Brainstem Server does not yet
  define those remote RPCs.
- `standUp()` and `sitDown()` confirm RPC acceptance, not physical completion.
- The client destructor performs a best-effort stop and may wait up to the
  configured RPC timeout when it still owns a lease.

The SDK version is `0.4.0`; its bundled protocol contract is `brainstem_api`
`2.2.0`.
