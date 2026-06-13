fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/tachiyomi.proto");
    prost_build::compile_protos(&["proto/tachiyomi.proto"], &["proto/"])?;
    Ok(())
}
