#![forbid(unsafe_code)]

use std::{
    env, fs,
    path::{Path, PathBuf},
};

const EVENTS: &str = "../proto/events";

fn protos(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("event definitions must be readable") {
        let path = entry.expect("event definitions must be readable").path();
        if path.is_dir() {
            protos(&path, found);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "proto")
        {
            found.push(path);
        }
    }
}

fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc must be available");
    std::env::set_var("PROTOC", protoc);
    println!("cargo:rerun-if-changed={EVENTS}");

    let mut files = Vec::new();
    protos(Path::new(EVENTS), &mut files);
    files.sort();
    let descriptors = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"))
        .join("event-descriptors.bin");
    prost_build::Config::new()
        .file_descriptor_set_path(&descriptors)
        .compile_well_known_types()
        .extern_path(".google.protobuf", "::pbjson_types")
        .enable_type_names()
        .compile_protos(&files, &["../proto"])
        .expect("event definitions must compile");
    pbjson_build::Builder::new()
        .register_descriptors(&fs::read(&descriptors).expect("descriptors were written"))
        .expect("descriptors must decode")
        .preserve_proto_field_names()
        .emit_fields()
        .build(&[".pingora.panel.events"])
        .expect("event JSON mappings must generate");
}
