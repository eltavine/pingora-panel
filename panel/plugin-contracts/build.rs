#![forbid(unsafe_code)]

use std::{env, fs, path::PathBuf};

fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc must be available");
    std::env::set_var("PROTOC", protoc);
    println!("cargo:rerun-if-changed=../proto/plugin");

    let descriptors = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"))
        .join("plugin-descriptors.bin");
    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .file_descriptor_set_path(&descriptors)
        .compile_protos(
            &[
                "../proto/plugin/v1/plugin.proto",
                "../proto/plugin/v1/ports.proto",
            ],
            &["../proto"],
        )
        .expect("the plugin protocol must compile");
    pbjson_build::Builder::new()
        .register_descriptors(&fs::read(&descriptors).expect("descriptors were written"))
        .expect("descriptors must decode")
        .preserve_proto_field_names()
        .build(&[
            ".pingora.panel.plugin.v1.Manifest",
            ".pingora.panel.plugin.v1.Resources",
        ])
        .expect("manifest JSON mappings must generate");
}
