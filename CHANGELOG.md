# Changelog

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
