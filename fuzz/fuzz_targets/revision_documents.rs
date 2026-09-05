#![no_main]

use libfuzzer_sys::fuzz_target;
use panel_config_state_codec::{decode_revision_state, decode_revision_transition_outcome};

fuzz_target!(|bytes: &[u8]| {
    let _ = decode_revision_state(bytes);
    let _ = decode_revision_transition_outcome(bytes);
});
