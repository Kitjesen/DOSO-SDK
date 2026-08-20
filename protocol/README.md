# Brainstem protocol snapshot

This directory vendors the `brainstem_api` protocol needed to build the DOSO
Brainstem C++ SDK. Keeping the two input `.proto` files here makes a clean SDK
checkout reproducible without the Brainstem Server monorepo or a Git submodule.

- Protocol version: `2.1.0`
- Upstream: <https://github.com/Kitjesen/brainstem_api>
- Base revision: `e252d4125c4ac5d045143148d999249d796e8f54`
- 2.1 `WalkChecked` contract: body-frame `x/y` are m/s and `z` is rad/s.

The SDK uses only the lease-aware `WalkChecked` motion path. Legacy unleased
`Walk` and queued `Command.walk` retain their existing normalized schema
semantics. The 2.1 addition does not change protobuf field numbers or wire
types. Update this snapshot only together with an SDK compatibility review and
tests.
