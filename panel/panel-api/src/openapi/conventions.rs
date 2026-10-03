use crate::{
    error_contract::{ERROR_STATUSES, PROBLEM_MEDIA_TYPE},
    request_context::{REQUEST_ID_HEADER, TRACEPARENT_HEADER, TRACESTATE_HEADER},
};
use utoipa::{
    openapi::{
        self,
        header::Header,
        path::{ParameterBuilder, ParameterIn},
        response::ResponseBuilder,
        schema::{ObjectBuilder, Schema, Type},
        Content, Ref, RefOr, Required,
    },
    Modify,
};

pub(super) struct HttpConventions;

/// Response enums whose values grow with the product, such as the blocks of
/// the configuration language; clients must accept values they do not know.
const EXTENSIBLE_ENUMS: &[&str] = &["Context"];

impl Modify for HttpConventions {
    fn modify(&self, document: &mut openapi::OpenApi) {
        if let Some(components) = document.components.as_mut() {
            for name in EXTENSIBLE_ENUMS {
                if let Some(RefOr::T(Schema::Object(object))) = components.schemas.get_mut(*name) {
                    if let Some(values) = object.enum_values.take() {
                        object
                            .extensions
                            .get_or_insert_default()
                            .insert("x-extensible-enum".into(), values.into());
                    }
                }
            }
        }
        for (path, item) in &mut document.paths.paths {
            item.parameters.get_or_insert_default().push(ParameterBuilder::new()
                .name(REQUEST_ID_HEADER).parameter_in(ParameterIn::Header)
                .required(Required::False)
                .description(Some("Optional request identity (1..=256 visible ASCII bytes). Missing, invalid or repeated values are replaced with a generated UUID."))
                .schema(Some(ObjectBuilder::new().schema_type(Type::String).min_length(Some(1)).max_length(Some(256))))
                .build());
            item.parameters.get_or_insert_default().push(ParameterBuilder::new()
                .name(TRACEPARENT_HEADER).parameter_in(ParameterIn::Header)
                .required(Required::False)
                .description(Some("W3C Trace Context parent of the caller's trace. Invalid values are ignored."))
                .schema(Some(ObjectBuilder::new().schema_type(Type::String).min_length(Some(55)).max_length(Some(512))))
                .build());
            item.parameters.get_or_insert_default().push(
                ParameterBuilder::new()
                    .name(TRACESTATE_HEADER)
                    .parameter_in(ParameterIn::Header)
                    .required(Required::False)
                    .description(Some(
                        "W3C Trace Context vendor state, propagated with a valid traceparent.",
                    ))
                    .schema(Some(
                        ObjectBuilder::new()
                            .schema_type(Type::String)
                            .max_length(Some(512)),
                    ))
                    .build(),
            );
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
                if path.starts_with("/api/v1/") && path != "/api/v1/openapi.json" {
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
                        let mut response = ResponseBuilder::new().description(description).content(
                            PROBLEM_MEDIA_TYPE,
                            Content::new(Some(Ref::from_schema_name("ProblemDetails"))),
                        );
                        if status == 503 {
                            response = response.header(
                                "Retry-After",
                                Header::new(ObjectBuilder::new().schema_type(Type::Integer)),
                            );
                        }
                        operation
                            .responses
                            .responses
                            .insert(status.to_string(), response.build().into());
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
