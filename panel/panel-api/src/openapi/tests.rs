use crate::ApiDoc;
use serde_json::{json, Value};
use utoipa::OpenApi;

fn document() -> Value {
    serde_json::to_value(ApiDoc::openapi()).unwrap()
}

#[test]
fn mutation_headers_describe_the_actual_request_requirements() {
    let doc = document();
    for path in ["/api/v1/gateway/prepare", "/api/v1/gateway/activate"] {
        let params = doc["paths"][path]["post"]["parameters"].as_array().unwrap();
        for name in ["x-actor", "x-deadline", "Idempotency-Key"] {
            let parameter = params.iter().find(|p| p["name"] == name).unwrap();
            assert_eq!(parameter["in"], "header");
            assert_eq!(parameter["required"], true);
        }
        let correlation = params
            .iter()
            .find(|p| p["name"] == "x-correlation-id")
            .unwrap();
        assert_eq!(correlation["required"], false);
        let request = &doc["paths"][path]["parameters"][0];
        assert_eq!(request["name"], "x-request-id");
        assert_eq!(request["required"], false);
    }
}

#[test]
fn error_responses_and_receipt_polling_match_the_wire_contract() {
    let doc = document();
    for (path, item) in doc["paths"].as_object().unwrap() {
        if !path.starts_with("/api/v1/gateway/") {
            continue;
        }
        let operation = item.get("post").or_else(|| item.get("get")).unwrap();
        for status in [
            "400", "401", "403", "404", "408", "409", "412", "413", "415", "422", "429", "500",
        ] {
            let response = &operation["responses"][status];
            assert_eq!(
                response["content"]["application/problem+json"]["schema"],
                json!({"$ref":"#/components/schemas/ProblemDetails"})
            );
            assert!(response["content"].get("application/json").is_none());
            assert!(response["headers"].get("x-request-id").is_some());
        }
    }
    assert!(
        doc["paths"]["/api/v1/gateway/receipts/{key}"]["get"]["responses"]["202"]["headers"]
            .get("Retry-After")
            .is_some()
    );
    assert!(
        doc["components"]["schemas"]["ActivateRequest"]["properties"]["expected_active_hash"]
            ["description"]
            .as_str()
            .unwrap()
            .contains("first activation")
    );
}

#[test]
fn published_openapi_matches_the_reviewed_contract() {
    let expected: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/openapi.json")).unwrap();
    assert_eq!(
        document(),
        expected,
        "Review the API change and regenerate with the export_openapi example"
    );
}
