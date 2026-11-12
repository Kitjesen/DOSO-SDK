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
    Ok(())
}
