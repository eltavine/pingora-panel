use crate::{
    error_contract::{ERROR_STATUSES, PROBLEM_MEDIA_TYPE},
    request_context::REQUEST_ID_HEADER,
};
use utoipa::{
    openapi::{
        self,
        header::Header,
        path::{ParameterBuilder, ParameterIn},
        response::ResponseBuilder,
        schema::{ObjectBuilder, Type},
        Content, Ref, RefOr, Required,
    },
    Modify,
};

pub(super) struct HttpConventions;

impl Modify for HttpConventions {
    fn modify(&self, document: &mut openapi::OpenApi) {
        for (path, item) in &mut document.paths.paths {
            item.parameters.get_or_insert_default().push(ParameterBuilder::new()
                .name(REQUEST_ID_HEADER).parameter_in(ParameterIn::Header)
                .required(Required::False)
                .description(Some("Optional request identity (1..=256 visible ASCII bytes). Missing, invalid or repeated values are replaced with a generated UUID."))
                .schema(Some(ObjectBuilder::new().schema_type(Type::String).min_length(Some(1)).max_length(Some(256))))
                .build());
            for operation in [
                &mut item.get,
                &mut item.post,
                &mut item.put,
                &mut item.patch,
                &mut item.delete,
                &mut item.options,
                &mut item.head,
                &mut item.trace,
            ]
            .into_iter()
            .flatten()
            {
                if path.starts_with("/api/v1/gateway/") {
                    // All application ports can return the stable shared error
                    // envelope. JSON extraction additionally returns 413/415/422.
                    for status in ERROR_STATUSES
                        .iter()
                        .map(|(_, status)| status.as_u16())
                        .chain([413, 415, 422, 500])
                    {
                        let description = axum::http::StatusCode::from_u16(status)
                            .ok()
                            .and_then(|value| value.canonical_reason())
                            .unwrap_or("Request failed");
                        operation.responses.responses.insert(
                            status.to_string(),
                            ResponseBuilder::new()
                                .description(description)
                                .content(
                                    PROBLEM_MEDIA_TYPE,
                                    Content::new(Some(Ref::from_schema_name("ProblemDetails"))),
                                )
                                .build()
                                .into(),
                        );
                    }
                }
                for response in operation.responses.responses.values_mut() {
                    if let RefOr::T(response) = response {
                        response
                            .headers
                            .insert(REQUEST_ID_HEADER.into(), Header::default());
                    }
                }
            }
        }
    }
}
