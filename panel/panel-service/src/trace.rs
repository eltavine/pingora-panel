use panel_context::TraceContext;
use tonic::metadata::{MetadataMap, MetadataValue};

/// The caller's W3C Trace Context from gRPC metadata. As for HTTP, an
/// invalid or repeated `traceparent` is ignored, and repeated `tracestate`
/// entries are combined in order.
pub fn trace_context(metadata: &MetadataMap) -> Option<TraceContext> {
    let mut parents = metadata.get_all("traceparent").iter();
    let parent = parents.next()?.to_str().ok()?;
    if parents.next().is_some() {
        return None;
    }
    let state = metadata
        .get_all("tracestate")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>()
        .join(",");
    TraceContext::parse(parent, (!state.is_empty()).then_some(state.as_str()))
}

/// Sends `trace` as `traceparent` and `tracestate` metadata.
pub fn propagate_trace(metadata: &mut MetadataMap, trace: Option<&TraceContext>) {
    let Some(trace) = trace else {
        return;
    };
    if let Ok(parent) = MetadataValue::try_from(trace.traceparent()) {
        metadata.insert("traceparent", parent);
        if let Some(Ok(state)) = trace.tracestate().map(MetadataValue::try_from) {
            metadata.insert("tracestate", state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn traces_round_trip_through_metadata() {
        let trace = TraceContext::parse(PARENT, Some("rojo=1")).unwrap();
        let mut metadata = MetadataMap::new();
        propagate_trace(&mut metadata, Some(&trace));
        assert_eq!(trace_context(&metadata), Some(trace));

        metadata.append("tracestate", "congo=2".parse().unwrap());
        assert_eq!(
            trace_context(&metadata).unwrap().tracestate(),
            Some("rojo=1,congo=2")
        );
        metadata.append("traceparent", PARENT.parse().unwrap());
        assert_eq!(trace_context(&metadata), None);

        let mut empty = MetadataMap::new();
        propagate_trace(&mut empty, None);
        assert!(empty.is_empty());
    }
}
