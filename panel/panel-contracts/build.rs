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
        // Debug for these is written by hand, without their secrets.
        .skip_debug([".pingora.panel.ops.v1.RegistryCredentials"])
        .boxed(".pingora.panel.automation.v1.BackupsDownloadResponse.message.backup")
        .boxed(".pingora.panel.gateway.v1.LuaSockets.tls")
        .compile_protos(
            &[
                "../proto/audit/v1/audit.proto",
                "../proto/automation/v1/backups.proto",
                "../proto/automation/v1/certificates.proto",
                "../proto/common/v1/common.proto",
                "../proto/config/v1/config.proto",
                "../proto/config/v1/configuration.proto",
                "../proto/io/cloudevents/v1/cloudevents.proto",
                "../proto/observability/v1/alerts.proto",
                "../proto/observability/v1/host.proto",
                "../proto/observability/v1/logs.proto",
                "../proto/observability/v1/traffic.proto",
                "../proto/ops/v1/agent.proto",
                "../proto/ops/v1/compose.proto",
                "../proto/ops/v1/containers.proto",
                "../proto/ops/v1/directories.proto",
                "../proto/ops/v1/engine_resources.proto",
                "../proto/ops/v1/gateway_service.proto",
                "../proto/ops/v1/images.proto",
                "../proto/ops/v1/listeners.proto",
                "../proto/gateway/v1/gateway.proto",
                "../proto/gateway/v1/runtime.proto",
                "../proto/platform/v1/platform.proto",
            ],
            &["../proto"],
        )
        .expect("panel protobuf contracts must compile");
}
