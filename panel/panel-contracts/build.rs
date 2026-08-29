fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc must be available");
    std::env::set_var("PROTOC", protoc);

    println!("cargo:rerun-if-changed=../proto");

    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_protos(
            &[
                "../proto/common/v1/common.proto",
                "../proto/gateway/v1/gateway.proto",
            ],
            &["../proto"],
        )
        .expect("panel protobuf contracts must compile");
}
