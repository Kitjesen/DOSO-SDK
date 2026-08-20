# DOSO Brainstem C++ SDK

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

This repository currently contains the C++ autonomous-motion client, not every
Brainstem service API.

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

The first release deliberately keeps the public surface small:

- `connect()` checks reachability and reads one control-state snapshot.
- `refresh()` acquires or renews the explicit control lease. Initial acquisition
  also sends and confirms a checked zero command.
- `move()` sends body-frame physical velocity: `x/y` in m/s and yaw in rad/s.
  It never converts values to `[-1, 1]` or silently clamps them.
- `stop()` sends a checked zero velocity and releases the lease. Only
  `result.confirmsStop()` means both operations were confirmed.
- `standUp()` and `sitDown()` stop and release locomotion first if this client
  owns the lease, then submit the posture action.
- `state()` returns the most recently decoded state without network I/O.

`move()` enforces the Server's 20 ms minimum checked-command interval. A lost
ACK is retried once with the same sequence and payload, matching the Server's
idempotency contract. Preemption, a rejected sequence, or a failed lease renewal
invalidates local motion readiness.

## Result model

Every network operation returns `brainstem::Result`:

| Field | Meaning |
| --- | --- |
| `ok` | The requested operation completed successfully. |
| `transport_ok` | The RPC reached the Server and produced an application-level response. |
| `accepted` | The Server accepted the request; it does not imply a physical posture transition completed. |
| `stop_confirmed` | Checked zero and lease release were both confirmed. |
| `state` | Latest normalized control state. |
| `error` | Stable reason details for logging and diagnostics. |

Use `confirmsStop()` for safety-sensitive stop decisions rather than checking
only `ok`.

## Control lease and identity

The client is serialized: call it from one control loop, or serialize calls in
the application. There is no hidden heartbeat thread. Call `refresh()` before
`state.lease_remaining_ms` reaches zero; accepted `move()` calls also renew the
Server lease. Lease expiry and remote-control preemption stop the Brainstem
motion path and make the client not ready.

`Config::client_id` is required. On hardware it must exactly match the
authenticated certificate principal configured by the Brainstem Server, using
the `name@scope` form such as `lingtu-driver@robot`.

Supplying any TLS file requires the CA, client certificate, and private key.
`tls.server_name` is an optional certificate-name override. An empty TLS
configuration is rejected unless the caller explicitly sets
`config.allow_insecure = true`; that opt-in is only for loopback development
and isolated tests. Production remote motion uses mutual TLS.

## Relationship to Unitree SDK2

| Unitree SDK2 | DOSO Brainstem C++ SDK |
| --- | --- |
| `SportClient::Init()` | construct `Client`, then `connect()` |
| internal control ownership | `refresh()` manages an explicit lease |
| `Move(vx, vy, wz)` | `move({vx_mps, vy_mps, yaw_rps})` |
| `StopMove()` | confirmed checked-zero plus lease release |
| `StandUp()` | `standUp()` |
| `Sit()` | `sitDown()` |
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

The repository vendors the exact `brainstem_api` 2.1 input files used for code
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

## Current limits

- No background status stream yet; `connect()` reads one snapshot.
- No IMU, joint, gesture, profile, camera, or motor-diagnostic facade yet.
- No recover-stand or damping action because Brainstem Server does not yet
  define those remote RPCs.
- `standUp()` and `sitDown()` confirm RPC acceptance, not physical completion.
- The client destructor performs a best-effort stop and may wait up to the
  configured RPC timeout when it still owns a lease.

The SDK version is `0.1.0`; its bundled protocol contract is `brainstem_api`
`2.1.0`.
