fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut protos = Vec::new();
    let mut directories = vec![std::path::PathBuf::from("../proto/doso")];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            if path.is_dir() {
                directories.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "proto")
            {
                protos.push(path);
            }
        }
    }
    protos.sort();
    tonic_prost_build::configure()
        .protoc_arg("--experimental_allow_proto3_optional")
        .compile_protos(&protos, &["../proto/doso".into()])?;
    println!("cargo:rerun-if-changed=../proto/doso");

    // Brainstem is a separate, lower-level protocol. Keep its generated
    // namespace distinct from the public DOSO seven-domain contract.
    let brainstem_protos = [
        "../proto/brainstem/brainstem_api/cms.proto",
        "../proto/brainstem/brainstem_api/common.proto",
        "../proto/brainstem/brainstem_api/external.proto",
        "../proto/brainstem/brainstem_api/recovery.proto",
    ];
    tonic_prost_build::configure()
        .protoc_arg("--experimental_allow_proto3_optional")
        .compile_protos(&brainstem_protos, &["../proto/brainstem"])?;
    println!("cargo:rerun-if-changed=../proto/brainstem");
    Ok(())
}
