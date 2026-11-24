//! The ProPresenter 19 schema as a committed descriptor set (see the crate docs).

use std::path::PathBuf;
use std::sync::OnceLock;

use prost_reflect::DescriptorPool;

/// The compiled schema (`proto/descriptor.bin`): `presentation.proto` and `template.proto` with everything they import.
pub static DESCRIPTOR_BIN: &[u8] = include_bytes!("../../../proto/descriptor.bin");

/// The schema, parsed once.
pub fn pool() -> &'static DescriptorPool {
    static POOL: OnceLock<DescriptorPool> = OnceLock::new();
    POOL.get_or_init(|| {
        DescriptorPool::decode(DESCRIPTOR_BIN)
            .expect("proto/descriptor.bin is a valid descriptor set")
    })
}

/// Where `proto/source/propresenter19` is in the repository (tests and the generator example use it).
pub fn source_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../proto/source/propresenter19")
}

/// Where the committed descriptor set is.
pub fn descriptor_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../proto/descriptor.bin")
}
