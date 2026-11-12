# Android point-cloud bridge

This standalone `cdylib` calls the DOSO Rust SDK and decodes its real PCLD v2
stream. Android owns the native point renderer. Points do not pass through
React Native JavaScript.

The JNI owner is `expo.modules.dosopointcloud.NativeCloud`:

```kotlin
external fun start(endpoint: String, kind: Int): Long
external fun poll(handle: Long): FloatArray?
external fun status(handle: Long): String
external fun stop(handle: Long)
```

`kind` is 1 for map display frames and 2 for scan display frames. `poll()`
returns interleaved XYZ in metres, `null` when there is no new frame, and an
empty array for scene reset. A reset marker survives later frames until polled;
changing the coordinate frame ID or scene epoch produces the same marker.
`poll()` delivers the empty array first, then the newest retained point array
on the next call so the native view can clear and refit its camera. Frames
replace the displayed cloud; they are not merged across coordinate systems.
The bridge stores at most one point array and one reset marker.
Colors remain available in the Rust SDK but this bridge sends XYZ for the
native height-colored renderer.

The native view calls these methods from its single worker. Handles are integer
registry keys. `stop()` removes the handle and signals the entire asynchronous
connection/subscription task, including a pending read; it does not wait for a
network timeout. Repeated stop is harmless.

Status is `connecting`, `waiting`, `live`, `stale`, `ended`, or
`error:<message>`. The first frame records its frame ID, epoch and sequence but
is not displayed. A later change of this identity is required before `live`.
`stale` means the frame identity has not advanced for three seconds. Repeated
identical frame identities do not refresh this timer. The renderer must clear
the visible cloud on non-live states. This verifies stream advancement, not
hardware sampling freshness. Source display sampling and quantization apply.

## Build on Windows

Install Rust 1.94+, `protoc`, the Android NDK matching the App, and the Rust
target. The script uses NDK clang directly and does not require `cargo-ndk`.

```powershell
rustup target add aarch64-linux-android
./android/build.ps1 -NdkRoot "$env:ANDROID_HOME/ndk/27.1.12297006" -OutputDir <module-jniLibs>
```

The output is `<module-jniLibs>/arm64-v8a/libdoso_sdk_android.so`. `-Abi x86_64`
requires the separate `x86_64-linux-android` Rust target and creates the
corresponding library. Other ABIs are not supported by this script. Do not
claim an App ABI is supported without packaging its matching native library.
`Cargo.lock` fixes this package's resolved dependencies.

## Connection boundary

The endpoint must be the actual SDK gRPC endpoint, commonly port 5052, rather
than the HTTP/WebSocket port 5050. Android must validate endpoint ownership and
its network policy before calling JNI. Rust sockets do not inherit OkHttp's
cleartext policy automatically. TLS follows the existing Rust SDK certificate
validation; this is not Android KeyChain or Brainstem mTLS integration.

On 2026-11-05, an isolated Android 13 arm64 instrumentation test received
changing simulated clouds through the actual LingTu Gateway and this Rust/JNI
bridge, and the App's OpenGL renderer produced non-background pixels in an
offscreen EGL buffer. The temporary test package and port mapping were removed.
This does not establish robot connectivity, installed App behavior, TLS
deployment, or G20 performance. Local evidence and reproduction steps are in
`.local/evidence/native-cloud-test/` (not part of the SDK package).
