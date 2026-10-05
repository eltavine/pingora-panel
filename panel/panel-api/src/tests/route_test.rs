use super::*;
use panel_application::{CommandContext, RequestScope};
use panel_config_api::{
    ApplyOutcome, ApplyRequest, ConfigurationChange, ConfigurationOutput, ConfigurationPort,
    ConfigurationQuery, DraftInfo, LanguageQuery,
};
use panel_domain::{NormalizedHost, PathPrefix, RevisionId, RouteId, SiteId, UpstreamPoolId};
use panel_ir::{
    DomainSpec, ListenerRef, RouteAction, RouteCondition, RouteMatcher, RouteSpec, RuntimeSnapshot,
    SiteSpec, ValueTest,
};
use serde_json::{json, Value};

/// A draft compiled to a snapshot with a canary route before the site's own.
struct Draft;

fn snapshot() -> RuntimeSnapshot {
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(3));
    let mut http = ListenerRef::new("http", "0.0.0.0:80");
    http.default_site_id = Some(SiteId::new("shop").unwrap());
    snapshot.listeners.push(http);
    snapshot.sites.push(SiteSpec::new(
        SiteId::new("shop").unwrap(),
        "shop",
        vec![DomainSpec::new(
            NormalizedHost::new("shop.example").unwrap(),
        )],
    ));
    let route = |id: &str, priority, path: &str| {
        RouteSpec::new(
            RouteId::new(id).unwrap(),
            SiteId::new("shop").unwrap(),
            priority,
            RouteMatcher::PathPrefix {
                path: PathPrefix::new(path).unwrap(),
            },
            RouteAction::Proxy {
                upstream_pool_id: UpstreamPoolId::new("app").unwrap(),
            },
        )
    };
    let mut canary = route("canary", 1, "/api");
    canary.name = Some("canary".into());
    canary.conditions = vec![RouteCondition::Header {
        name: "x-canary".into(),
        test: ValueTest::Equals {
            value: "1".into(),
            ignore_case: false,
        },
    }];
    snapshot.routes = vec![canary, route("api", 5, "/api")];
    snapshot
}

#[async_trait]
impl ConfigurationPort for Draft {
    async fn read(
        &self,
        _: RequestScope,
        query: ConfigurationQuery,
    ) -> Result<ConfigurationOutput> {
        let ConfigurationQuery::Language(LanguageQuery::Ir) = query else {
            return Err(PanelError::unavailable("only the snapshot is read here"));
        };
        Ok(ConfigurationOutput {
            content: serde_json::to_vec(&snapshot()).unwrap(),
            etag: None,
            draft: DraftInfo {
                version: 9,
                ..DraftInfo::default()
            },
        })
    }

    async fn change(
        &self,
        _: CommandContext,
        _: ConfigurationChange,
    ) -> Result<ConfigurationOutput> {
        Err(PanelError::unavailable("nothing changes here"))
    }

    async fn apply(&self, _: CommandContext, _: ApplyRequest) -> Result<ApplyOutcome> {
        Err(PanelError::unavailable("nothing is applied here"))
    }
}

async fn tested(request: Value) -> (StatusCode, Value) {
    let app = router(
        ApiState::new(Arc::new(GatewayService::new(
            Arc::new(FakeGateway),
            Arc::new(IdentityCompiler),
        )))
        .with_configuration(Arc::new(Draft)),
    );
    let response = app
        .oneshot(
            Request::post("/api/v1/config/route-test")
                .header("content-type", "application/json")
                .body(Body::from(request.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn the_route_a_request_takes_is_explained() {
    let (status, result) = tested(json!({
        "host": "Shop.Example:8080",
        "target": "/api/items?tag=new",
        "headers": [{ "name": "X-Canary", "value": "0" }],
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["outcome"], "routed");
    assert_eq!(result["draft_version"], 9);
    assert_eq!(result["host"], "shop.example");
    assert_eq!(result["path"], "/api/items");
    assert_eq!(result["site_id"], "shop");
    assert_eq!(result["route_id"], "api");
    assert_eq!(
        result["routes"],
        json!([
            {
                "route_id": "canary", "name": "canary", "matched": false,
                "reason": "header x-canary = \"1\" (it is \"0\") does not hold",
            },
            { "route_id": "api", "matched": true },
        ])
    );

    let (_, canary) = tested(json!({
        "host": "shop.example",
        "target": "/api",
        "headers": [{ "name": "x-canary", "value": "1" }],
    }))
    .await;
    assert_eq!(canary["route_id"], "canary");
    assert_eq!(canary["routes"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn requests_no_site_or_route_takes_say_so() {
    let (_, no_site) = tested(json!({ "host": "other.example" })).await;
    assert_eq!(no_site["outcome"], "no_site");
    assert!(no_site.get("site_id").is_none());

    let (_, default) =
        tested(json!({ "host": "other.example", "listener": "http", "target": "/api" })).await;
    assert_eq!(default["outcome"], "routed");
    assert_eq!(default["default_site"], true);

    let (_, no_route) = tested(json!({ "host": "shop.example", "target": "/home" })).await;
    assert_eq!(no_route["outcome"], "no_route");
    assert_eq!(
        no_route["routes"][1]["reason"],
        "the path is not prefix /api"
    );

    let (status, problem) = tested(json!({ "host": "shop.example", "target": "api" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    let (status, problem) = tested(json!({ "host": "shop.example", "client": "nowhere" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
}
