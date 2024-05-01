use std::io::Result;
use std::path::{Path, PathBuf};

fn list_files(dir: &Path) -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(&dir)
        .unwrap()
        .into_iter()
        .filter_map(|res| {
            let res = res.unwrap();
            res.file_type().unwrap().is_file().then_some(res.path())
        })
}

fn main() -> Result<()> {
    // Doesn't support all architectures (namely, M1 macs), so for now we can't just unwrap it.
    if let Ok(protoc) = protoc_bin_vendored::protoc_bin_path() {
        std::env::set_var("PROTOC", protoc);
    }

    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let generated = crate_root.join("src/generated");
    let input_dir = crate_root.join("proto");
    let proto_files: Vec<PathBuf> = list_files(&input_dir.join("rpc"))
        .chain(list_files(&input_dir.join("objects")))
        .collect();
    let service_files: Vec<PathBuf> = list_files(&input_dir.join("services")).collect();

    prost_build::Config::new()
        .out_dir(&generated)
        .include_file(generated.join("mod.rs"))
        .compile_protos(&proto_files, &[&input_dir])
        .unwrap();

    tonic_build::configure()
        .out_dir(&generated)
        .build_client(true)
        .build_server(true)
        .compile(&service_files, &[&input_dir])
        .unwrap();

    Ok(())
}
