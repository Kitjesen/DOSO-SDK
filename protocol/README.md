# Brainstem protocol snapshot

This directory vendors the `brainstem_api` protocol needed to build the DOSO
SDK Brainstem C++ client. Keeping the two input `.proto` files here makes a clean SDK
checkout reproducible without the Brainstem Server monorepo or a Git submodule.

- Protocol version: `2.2.0`
- Upstream: <https://github.com/Kitjesen/brainstem_api>
- 2.1 `WalkChecked` contract: body-frame `x/y` are m/s and `z` is rad/s.
- 2.2 `Imu.linear_acceleration` is optional, body-frame acceleration in m/s².
  Older 2.x servers remain readable and produce an empty optional value.

The SDK uses only the lease-aware `WalkChecked` motion path. Legacy unleased
`Walk` and queued `Command.walk` use the same physical-unit convention but are
not called by this SDK. Update this snapshot only together with an SDK
compatibility review and tests.
