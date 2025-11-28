use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Specify the directory containing your Protobuf schema files
    let schema_dir = Path::new("proto/");
    let mut schemas = vec![];

    // Add all .proto files from the schema directory
    for entry in std::fs::read_dir(schema_dir)
        .expect(format!("Failed to read {} directory", schema_dir.display()).as_str())
    {
        let entry = entry.expect("Failed to read directory entry");
        let path = entry.path();

        if path.extension().map_or(false, |ext| ext == "proto") {
            schemas.push(path);
        }
    }

    // Tell Cargo to re-run this build script if any .proto file changes
    println!("cargo:rerun-if-changed={}", schema_dir.to_str().unwrap());

    // Generate Rust code
    prost_build::compile_protos(&schemas, &[schema_dir.to_str().unwrap()])?;

    Ok(())
}
