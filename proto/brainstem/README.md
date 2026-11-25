# Brainstem protocol snapshot

This directory vendors the `brainstem_api` protocol needed to build the DOSO
SDK Brainstem Rust and C++ clients. Keeping the input `.proto` files here makes
a clean SDK checkout reproducible without the Brainstem Server monorepo or a
Git submodule.

- Protocol version: `v0.0.1` (`0.0.1`, unified with Brainstem `v0.0.1`)
- Upstream: <https://github.com/Kitjesen/brainstem_api>
- `2.1` `WalkChecked`: body-frame `x/y` are m/s and `z` is rad/s.
- `2.2` `Imu.linear_acceleration`: optional body-frame acceleration in m/s².
  Older 2.x servers remain readable and produce an empty optional value.
- `2.3` `RobotControl` & `RobotRecovery`: adds `ProfileInfo.mode_capabilities`
  and `active_mode_id`, `ListGestures` / `PlayGesture`, `GetCmsState` /
  `ListenCmsState`, and `recovery.proto` (`RobotRecovery`, `RobotModeCapability`),
  vendored alongside `external.proto` (`RobotExternalControl`).
- `2.4` `RobotControl.Damp`: soft emergency-stop RPC (`Damp(Empty) -> Empty`),
  `CmsStateKind.CMS_STATE_KIND_DAMPING = 5`, and `Command.damp = 5`.

The SDK uses only the lease-aware `WalkChecked` motion path. Legacy unleased
`Walk` and queued `Command.walk` use the same physical-unit convention but are
not called by this SDK. Update this snapshot only together with an SDK
compatibility review and tests.
