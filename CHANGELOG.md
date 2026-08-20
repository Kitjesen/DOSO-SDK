# Changelog

## 0.1.0

- Added the DOSO Brainstem C++ autonomous-motion client.
- Added explicit lease acquire, renewal, and release handling.
- Added checked physical-velocity commands with sequence validation and one
  idempotent retry after a lost ACK.
- Added confirmed stop, stand-up, sit-down, normalized control state, and mTLS.
- Made mTLS the default and insecure transport an explicit development opt-in.
- Preserved a stop-only recovery path after transient command rejection,
  sequence mismatch, or ambiguous transport failure.
- Added a self-contained Brainstem API 2.1 protocol snapshot and CMake package.
