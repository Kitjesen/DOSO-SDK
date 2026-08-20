# Changelog

## 0.4.0

- Added optional body-frame IMU linear acceleration in m/s² with backward
  compatibility for Brainstem 2.1 servers.
- Added motor enable/disable, joint-zero, and selective/all-motor fault-clear
  operations without exposing gRPC types.
- Renamed the public CMS vocabulary to `BodyState`; CMS remains only an
  internal Brainstem protocol name.
- Added direct latest IMU, joint, and body-state cache getters.
- Added Brainstem Server start-time access and runnable telemetry and
  maintenance examples.
- Updated the bundled protocol snapshot to `brainstem_api` 2.2.0.

## 0.3.0

- Added managed, selectively enabled IMU, joint, and CMS telemetry streams.
- Added thread-safe latest-value snapshots with IMU age/freshness and per-joint
  validity/freshness masks.
- Added merging of full joint snapshots and single-joint delta updates into one
  stable 16-joint view.
- Kept raw cancellable subscriptions available for applications that need to
  process every frame.

## 0.2.0

- Added typed CMS-state, motor-diagnostic, and voltage snapshot queries.
- Added cancellable CMS-state, IMU, and joint-state subscriptions without
  exposing gRPC or Protobuf in public headers.
- Preserved both Brainstem joint stream forms: 16-joint snapshots and
  single-joint updates.
- Added accepted/rejected command counters to the normalized control state.

## 0.1.0

- Added the DOSO SDK Brainstem C++ autonomous-motion client.
- Added explicit lease acquire, renewal, and release handling.
- Added checked physical-velocity commands with sequence validation and one
  idempotent retry after a lost ACK.
- Added confirmed stop, stand-up, sit-down, normalized control state, and mTLS.
- Made mTLS the default and insecure transport an explicit development opt-in.
- Preserved a stop-only recovery path after transient command rejection,
  sequence mismatch, or ambiguous transport failure.
- Added a self-contained Brainstem API 2.1 protocol snapshot and CMake package.
