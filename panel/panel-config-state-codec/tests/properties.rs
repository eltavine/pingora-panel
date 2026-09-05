#![forbid(unsafe_code)]

use panel_config_state_codec::{
    decode_revision_state, decode_revision_transition_outcome, DEFAULT_MAX_REVISION_DOCUMENT_BYTES,
};
use proptest::prelude::*;

proptest! {
    #[test]
    fn arbitrary_revision_documents_never_panic(
        bytes in prop::collection::vec(any::<u8>(), 0..=DEFAULT_MAX_REVISION_DOCUMENT_BYTES + 8),
    ) {
        let _ = decode_revision_state(&bytes);
        let _ = decode_revision_transition_outcome(&bytes);
    }
}
