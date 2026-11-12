param(
  [string]$NdkRoot = $env:ANDROID_NDK_HOME,
  [string]$OutputDir = (Join-Path $PSScriptRoot 'jniLibs'),
  [ValidateSet('arm64-v8a', 'x86_64')][string]$Abi = 'arm64-v8a'
)

$ErrorActionPreference = 'Stop'
if (-not $NdkRoot) { $NdkRoot = $env:ANDROID_NDK_ROOT }
if (-not $NdkRoot -and $env:ANDROID_HOME) {
  $NdkRoot = Join-Path $env:ANDROID_HOME 'ndk/27.1.12297006'
}
if (-not $NdkRoot) { throw 'Set ANDROID_NDK_HOME or pass -NdkRoot.' }
$target = if ($Abi -eq 'arm64-v8a') { 'aarch64-linux-android' } else { 'x86_64-linux-android' }
$installed = & rustup target list --installed
if ($LASTEXITCODE -ne 0 -or $target -notin $installed) {
  throw "Install the Rust target before building: rustup target add $target"
}
$llvm = Join-Path $NdkRoot 'toolchains/llvm/prebuilt/windows-x86_64/bin'
$clang = Join-Path $llvm "$($target)24-clang.cmd"
$archive = Join-Path $llvm 'llvm-ar.exe'
if (-not (Test-Path -LiteralPath $clang) -or -not (Test-Path -LiteralPath $archive)) {
  throw "The NDK toolchain is unavailable: $llvm"
}
$suffix = $target.Replace('-', '_')
$values = @{
  "CARGO_TARGET_$($suffix.ToUpperInvariant())_LINKER" = $clang
  "CC_$suffix" = $clang
  "AR_$suffix" = $archive
}
$prior = @{}
try {
  foreach ($name in $values.Keys) {
    $prior[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
    [Environment]::SetEnvironmentVariable($name, $values[$name], 'Process')
  }
  & cargo build --manifest-path (Join-Path $PSScriptRoot 'Cargo.toml') --target-dir (Join-Path $PSScriptRoot 'target') --locked --release --target $target
  if ($LASTEXITCODE -ne 0) { throw "Rust Android build failed ($LASTEXITCODE)." }
  $source = Join-Path $PSScriptRoot "target/$target/release/libdoso_sdk_android.so"
  $destination = Join-Path $OutputDir $Abi
  New-Item -ItemType Directory -Path $destination -Force | Out-Null
  Copy-Item -LiteralPath $source -Destination (Join-Path $destination 'libdoso_sdk_android.so') -Force
  Write-Output (Join-Path $destination 'libdoso_sdk_android.so')
} finally {
  foreach ($name in $prior.Keys) {
    [Environment]::SetEnvironmentVariable($name, $prior[$name], 'Process')
  }
}
