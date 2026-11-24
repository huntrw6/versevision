//! `proto/descriptor.bin` is what `proto/source/propresenter19/*.proto` compile to, so a consumer needs no `protoc` and no build script, and the
//! schema cannot drift from the sources. If this fails: `cargo run -p versevision-pp --example gen_descriptors`, and commit the file.

use prost::Message;

#[test]
fn committed_descriptor_set_is_what_the_sources_compile_to() {
    let set = protox::compile(
        ["presentation.proto", "template.proto"],
        [versevision_pp::source_dir()],
    )
    .expect("compile the schema");
    let committed = std::fs::read(versevision_pp::descriptor_path()).expect("proto/descriptor.bin");
    assert!(set.encode_to_vec() == committed, "proto/descriptor.bin is out of date: run `cargo run -p versevision-pp --example gen_descriptors`");
}

#[test]
fn the_schema_has_no_map_fields_the_codec_would_have_to_order() {
    // `pb.rs` does not decode map fields (Python writes them with sorted keys when deterministic); the two roots must not have any
    for name in ["rv.data.Presentation", "rv.data.Template.Document"] {
        let mut pending = vec![versevision_pp::pool().get_message_by_name(name).unwrap()];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(message) = pending.pop() {
            if !seen.insert(message.full_name().to_string()) {
                continue;
            }
            for field in message.fields() {
                assert!(!field.is_map(), "{} is a map field", field.full_name());
                if let prost_reflect::Kind::Message(inner) = field.kind() {
                    pending.push(inner);
                }
            }
        }
    }
}
