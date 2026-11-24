//! Rewrites `proto/descriptor.bin` from `proto/source/propresenter19/*.proto` (pure Rust, no `protoc`).
//! Run: `cargo run -p versevision-pp --example gen_descriptors`
//! The test `tests/descriptor.rs` fails when the committed file is not what the sources compile to.

use prost::Message;

fn main() {
    let set = protox::compile(
        ["presentation.proto", "template.proto"],
        [versevision_pp::source_dir()],
    )
    .expect("compile the schema");
    let path = versevision_pp::descriptor_path();
    std::fs::write(&path, set.encode_to_vec()).expect("write descriptor.bin");
    println!("wrote {}", path.display());
}
