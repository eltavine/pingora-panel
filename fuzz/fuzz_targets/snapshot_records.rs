#![no_main]

use libfuzzer_sys::fuzz_target;
use snapshot_store_fs::{JsonSnapshotRecordCodecV1, SnapshotRecordCodec};

fuzz_target!(|record: &[u8]| {
    let codec = JsonSnapshotRecordCodecV1;
    let _ = codec.decode_payload(record);
});
