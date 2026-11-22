fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut protos = Vec::new();
    let mut directories = vec![std::path::PathBuf::from("proto")];
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
    tonic_prost_build::configure().compile_protos(&protos, &["proto".into()])?;
    println!("cargo:rerun-if-changed=proto");

    // Brainstem is a separate, lower-level protocol. Keep its generated
    // namespace distinct from the public DOSO seven-domain contract.
    let brainstem_protos = [
        "brainstem/protocol/brainstem_api/cms.proto",
        "brainstem/protocol/brainstem_api/common.proto",
        "brainstem/protocol/brainstem_api/external.proto",
        "brainstem/protocol/brainstem_api/recovery.proto",
    ];
    tonic_prost_build::configure().compile_protos(&brainstem_protos, &["brainstem/protocol"])?;
    println!("cargo:rerun-if-changed=brainstem/protocol");
    Ok(())
}
