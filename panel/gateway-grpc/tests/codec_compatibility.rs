#![forbid(unsafe_code)]

// This is a downstream crate: removing an established public function must fail to
// compile even if every internal call site has already migrated to the codec.
use gateway_grpc::{decode_snapshot, encode_snapshot};
use panel_domain::RevisionId;
use panel_ir::RuntimeSnapshot;

#[test]
fn established_codec_imports_preserve_the_same_snapshot() {
    let snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    let wire = encode_snapshot(&snapshot);
    assert_eq!(wire, gateway_proto_codec::encode_snapshot(&snapshot));
    assert_eq!(decode_snapshot(wire).unwrap(), snapshot);
}
