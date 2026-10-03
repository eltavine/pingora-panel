#![forbid(unsafe_code)]

fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc must be available");
    std::env::set_var("PROTOC", protoc);

    println!("cargo:rerun-if-changed=../proto");

    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        // Ordered attributes keep encoded CloudEvents byte-stable for storage
        // fixtures and content hashes.
        .btree_map(".io.cloudevents.v1.CloudEvent.attributes")
        .compile_protos(
            &[
                "../proto/common/v1/common.proto",
                "../proto/config/v1/config.proto",
                "../proto/config/v1/configuration.proto",
                "../proto/io/cloudevents/v1/cloudevents.proto",
                "../proto/gateway/v1/gateway.proto",
                "../proto/platform/v1/platform.proto",
            ],
            &["../proto"],
        )
        .expect("panel protobuf contracts must compile");
}
