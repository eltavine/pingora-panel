use super::DEFAULT_REQUEST_TIMEOUT;
use chrono::DateTime;
use panel_contracts::common::v1 as common;
use panel_engine::GatewayRequestMetadata;
use panel_errors::{PanelError, Result};
use std::{
    future::Future,
    num::NonZeroUsize,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const DEFAULT_MAX_REQUEST_ID_BYTES: usize = 128;
pub const DEFAULT_MAX_CORRELATION_ID_BYTES: usize = 128;
pub const DEFAULT_MAX_ACTOR_BYTES: usize = 320;
pub const DEFAULT_MAX_DEADLINE_BYTES: usize = 64;
pub const DEFAULT_MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;
pub const DEFAULT_MAX_SCHEMA_VERSION_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RequestClass {
    ReadOnly,
    Mutation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DeadlineRequirement {
    Optional,
    Mutations,
    All,
}

/// Resource limits for transport metadata copied into policies and events.
///
/// Private fields and stable accessors allow future metadata fields to acquire
/// independent defaults without exposing a constructible public struct layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GatewayRequestMetadataLimits {
    request_id_bytes: NonZeroUsize,
    correlation_id_bytes: NonZeroUsize,
    actor_bytes: NonZeroUsize,
    deadline_bytes: NonZeroUsize,
    idempotency_key_bytes: NonZeroUsize,
    schema_version_bytes: NonZeroUsize,
}

impl GatewayRequestMetadataLimits {
    pub fn new(
        request_id_bytes: usize,
        correlation_id_bytes: usize,
        actor_bytes: usize,
        deadline_bytes: usize,
        idempotency_key_bytes: usize,
        schema_version_bytes: usize,
    ) -> Result<Self> {
        let non_zero = |value, field| {
            NonZeroUsize::new(value).ok_or_else(|| {
                PanelError::invalid_argument(format!(
                    "gateway request metadata limit for {field} must be non-zero"
                ))
            })
        };
        Ok(Self {
            request_id_bytes: non_zero(request_id_bytes, "request_id")?,
            correlation_id_bytes: non_zero(correlation_id_bytes, "correlation_id")?,
            actor_bytes: non_zero(actor_bytes, "actor")?,
            deadline_bytes: non_zero(deadline_bytes, "deadline")?,
            idempotency_key_bytes: non_zero(idempotency_key_bytes, "idempotency_key")?,
            schema_version_bytes: non_zero(schema_version_bytes, "schema_version")?,
        })
    }

    pub fn request_id_bytes(self) -> usize {
        self.request_id_bytes.get()
    }

    pub fn correlation_id_bytes(self) -> usize {
        self.correlation_id_bytes.get()
    }

    pub fn actor_bytes(self) -> usize {
        self.actor_bytes.get()
    }

    pub fn deadline_bytes(self) -> usize {
        self.deadline_bytes.get()
    }

    pub fn idempotency_key_bytes(self) -> usize {
        self.idempotency_key_bytes.get()
    }

    pub fn schema_version_bytes(self) -> usize {
        self.schema_version_bytes.get()
    }
}

impl Default for GatewayRequestMetadataLimits {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_REQUEST_ID_BYTES,
            DEFAULT_MAX_CORRELATION_ID_BYTES,
            DEFAULT_MAX_ACTOR_BYTES,
            DEFAULT_MAX_DEADLINE_BYTES,
            DEFAULT_MAX_IDEMPOTENCY_KEY_BYTES,
            DEFAULT_MAX_SCHEMA_VERSION_BYTES,
        )
        .expect("default gateway request metadata limits are non-zero")
    }
}

#[derive(Clone, Copy)]
pub struct RequestPolicyContext<'a> {
    context: &'a common::RequestContext,
    class: RequestClass,
}

impl<'a> RequestPolicyContext<'a> {
    pub fn request_context(self) -> &'a common::RequestContext {
        self.context
    }

    pub fn class(self) -> RequestClass {
        self.class
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestExecutionBudget {
    timeout: Duration,
}

impl RequestExecutionBudget {
    pub fn new(timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(PanelError::invalid_argument(
                "gateway request budget must be non-zero",
            ));
        }
        Ok(Self { timeout })
    }

    pub fn timeout(self) -> Duration {
        self.timeout
    }

    pub async fn execute<T>(self, future: impl Future<Output = Result<T>>) -> Result<T> {
        tokio::time::timeout(self.timeout, future)
            .await
            .map_err(|_| PanelError::deadline_exceeded("gateway request deadline exceeded"))?
    }
}

pub trait GatewayRequestRule: Send + Sync {
    fn validate(&self, request: RequestPolicyContext<'_>) -> Result<()>;
}

pub trait GatewayRequestBudgetPolicy: Send + Sync {
    fn budget(&self, request: RequestPolicyContext<'_>) -> Result<RequestExecutionBudget>;
}

pub trait GatewayRequestPolicy: Send + Sync {
    fn validate(
        &self,
        context: Option<&common::RequestContext>,
        class: RequestClass,
    ) -> Result<RequestExecutionBudget>;
}

pub struct CompositeGatewayRequestPolicy {
    rules: Vec<Arc<dyn GatewayRequestRule>>,
    budget_policy: Arc<dyn GatewayRequestBudgetPolicy>,
}

impl CompositeGatewayRequestPolicy {
    pub fn new(budget_policy: Arc<dyn GatewayRequestBudgetPolicy>) -> Self {
        Self {
            rules: Vec::new(),
            budget_policy,
        }
    }

    pub fn with_rule(mut self, rule: Arc<dyn GatewayRequestRule>) -> Self {
        self.rules.push(rule);
        self
    }
}

impl GatewayRequestPolicy for CompositeGatewayRequestPolicy {
    fn validate(
        &self,
        context: Option<&common::RequestContext>,
        class: RequestClass,
    ) -> Result<RequestExecutionBudget> {
        let context =
            context.ok_or_else(|| PanelError::invalid_argument("request context is required"))?;
        let request = RequestPolicyContext { context, class };
        for rule in &self.rules {
            rule.validate(request)?;
        }
        self.budget_policy.budget(request)
    }
}

#[derive(Default)]
pub struct RequiredRequestIdentityRule;

impl GatewayRequestRule for RequiredRequestIdentityRule {
    fn validate(&self, request: RequestPolicyContext<'_>) -> Result<()> {
        if request.context.request_id.is_empty() {
            return Err(PanelError::invalid_argument("request_id is required"));
        }
        if request.context.actor.is_empty() {
            return Err(PanelError::invalid_argument("actor is required"));
        }
        Ok(())
    }
}

pub struct BoundedGatewayRequestMetadataRule {
    limits: GatewayRequestMetadataLimits,
}

impl BoundedGatewayRequestMetadataRule {
    pub fn new(limits: GatewayRequestMetadataLimits) -> Self {
        Self { limits }
    }

    pub fn limits(&self) -> GatewayRequestMetadataLimits {
        self.limits
    }
}

impl Default for BoundedGatewayRequestMetadataRule {
    fn default() -> Self {
        Self::new(GatewayRequestMetadataLimits::default())
    }
}

impl GatewayRequestRule for BoundedGatewayRequestMetadataRule {
    fn validate(&self, request: RequestPolicyContext<'_>) -> Result<()> {
        let context = request.request_context();
        validate_metadata_field(
            "request_id",
            &context.request_id,
            self.limits.request_id_bytes(),
        )?;
        validate_metadata_field(
            "correlation_id",
            &context.correlation_id,
            self.limits.correlation_id_bytes(),
        )?;
        validate_metadata_field("actor", &context.actor, self.limits.actor_bytes())?;
        validate_metadata_field("deadline", &context.deadline, self.limits.deadline_bytes())?;
        validate_metadata_field(
            "idempotency_key",
            &context.idempotency_key,
            self.limits.idempotency_key_bytes(),
        )?;
        validate_metadata_field(
            "schema_version",
            &context.schema_version,
            self.limits.schema_version_bytes(),
        )
    }
}

fn validate_metadata_field(name: &str, value: &str, max_bytes: usize) -> Result<()> {
    if value.len() > max_bytes {
        return Err(PanelError::resource_exhausted(format!(
            "{name} exceeds the {max_bytes} byte request metadata limit"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(PanelError::invalid_argument(format!(
            "{name} must not contain control characters"
        )));
    }
    Ok(())
}

pub(crate) fn project_gateway_event_metadata(
    context: Option<&common::RequestContext>,
    limits: GatewayRequestMetadataLimits,
) -> GatewayRequestMetadata {
    let Some(context) = context else {
        return GatewayRequestMetadata::new("", "", "");
    };
    GatewayRequestMetadata::new(
        project_metadata_field(&context.request_id, limits.request_id_bytes()),
        project_metadata_field(&context.correlation_id, limits.correlation_id_bytes()),
        project_metadata_field(&context.actor, limits.actor_bytes()),
    )
}

fn project_metadata_field(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes && !value.chars().any(char::is_control) {
        value.to_owned()
    } else {
        String::new()
    }
}

pub struct SupportedProtocolVersionsRule {
    versions: Arc<[String]>,
}

impl SupportedProtocolVersionsRule {
    pub fn new(versions: impl IntoIterator<Item = impl Into<String>>) -> Result<Self> {
        let versions: Arc<[String]> = versions.into_iter().map(Into::into).collect();
        if versions.is_empty() || versions.iter().any(String::is_empty) {
            return Err(PanelError::invalid_argument(
                "supported protocol versions must contain non-empty values",
            ));
        }
        Ok(Self { versions })
    }
}

impl GatewayRequestRule for SupportedProtocolVersionsRule {
    fn validate(&self, request: RequestPolicyContext<'_>) -> Result<()> {
        if self
            .versions
            .iter()
            .any(|version| version == &request.context.schema_version)
        {
            return Ok(());
        }
        Err(PanelError::unsupported_capability(format!(
            "request schema {} is not supported",
            request.context.schema_version
        )))
    }
}

#[derive(Default)]
pub struct MutationIdempotencyRule;

impl GatewayRequestRule for MutationIdempotencyRule {
    fn validate(&self, request: RequestPolicyContext<'_>) -> Result<()> {
        if request.class == RequestClass::Mutation && request.context.idempotency_key.is_empty() {
            return Err(PanelError::invalid_argument(
                "idempotency_key is required for mutations",
            ));
        }
        Ok(())
    }
}

pub trait DeadlineClock: Send + Sync {
    fn now(&self) -> SystemTime;
}

#[derive(Default)]
pub struct SystemDeadlineClock;

impl DeadlineClock for SystemDeadlineClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

pub struct DeadlineRequestBudgetPolicy {
    maximum_duration: Duration,
    deadline_requirement: DeadlineRequirement,
    clock: Arc<dyn DeadlineClock>,
}

impl DeadlineRequestBudgetPolicy {
    pub fn new(
        maximum_duration: Duration,
        deadline_requirement: DeadlineRequirement,
    ) -> Result<Self> {
        Self::with_clock(
            maximum_duration,
            deadline_requirement,
            Arc::new(SystemDeadlineClock),
        )
    }

    pub fn with_clock(
        maximum_duration: Duration,
        deadline_requirement: DeadlineRequirement,
        clock: Arc<dyn DeadlineClock>,
    ) -> Result<Self> {
        RequestExecutionBudget::new(maximum_duration)?;
        Ok(Self {
            maximum_duration,
            deadline_requirement,
            clock,
        })
    }

    fn deadline_required(&self, class: RequestClass) -> bool {
        matches!(self.deadline_requirement, DeadlineRequirement::All)
            || matches!(
                (self.deadline_requirement, class),
                (DeadlineRequirement::Mutations, RequestClass::Mutation)
            )
    }

    fn remaining(&self, value: &str) -> Result<Duration> {
        let parsed = DateTime::parse_from_rfc3339(value).map_err(|error| {
            PanelError::invalid_argument(format!("deadline must be RFC 3339: {error}"))
        })?;
        let deadline_millis = i128::from(parsed.timestamp_millis());
        let now_millis = self
            .clock
            .now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i128)
            .unwrap_or(i128::MIN);
        let remaining = deadline_millis - now_millis;
        if remaining <= 0 {
            return Err(PanelError::deadline_exceeded(
                "request context deadline has expired",
            ));
        }
        let remaining = u64::try_from(remaining).unwrap_or(u64::MAX);
        Ok(Duration::from_millis(remaining).min(self.maximum_duration))
    }
}

impl GatewayRequestBudgetPolicy for DeadlineRequestBudgetPolicy {
    fn budget(&self, request: RequestPolicyContext<'_>) -> Result<RequestExecutionBudget> {
        let timeout = if request.context.deadline.is_empty() {
            if self.deadline_required(request.class) {
                return Err(PanelError::invalid_argument("deadline is required"));
            }
            self.maximum_duration
        } else {
            self.remaining(&request.context.deadline)?
        };
        RequestExecutionBudget::new(timeout)
    }
}

pub struct StandardGatewayRequestPolicy {
    inner: CompositeGatewayRequestPolicy,
}

impl StandardGatewayRequestPolicy {
    pub fn new(
        maximum_duration: Duration,
        deadline_requirement: DeadlineRequirement,
    ) -> Result<Self> {
        Self::with_metadata_limits(
            maximum_duration,
            deadline_requirement,
            GatewayRequestMetadataLimits::default(),
        )
    }

    pub fn with_metadata_limits(
        maximum_duration: Duration,
        deadline_requirement: DeadlineRequirement,
        metadata_limits: GatewayRequestMetadataLimits,
    ) -> Result<Self> {
        Self::with_clock_and_metadata_limits(
            maximum_duration,
            deadline_requirement,
            Arc::new(SystemDeadlineClock),
            metadata_limits,
        )
    }

    pub fn with_clock(
        maximum_duration: Duration,
        deadline_requirement: DeadlineRequirement,
        clock: Arc<dyn DeadlineClock>,
    ) -> Result<Self> {
        Self::with_clock_and_metadata_limits(
            maximum_duration,
            deadline_requirement,
            clock,
            GatewayRequestMetadataLimits::default(),
        )
    }

    pub fn with_clock_and_metadata_limits(
        maximum_duration: Duration,
        deadline_requirement: DeadlineRequirement,
        clock: Arc<dyn DeadlineClock>,
        metadata_limits: GatewayRequestMetadataLimits,
    ) -> Result<Self> {
        let budget_policy = Arc::new(DeadlineRequestBudgetPolicy::with_clock(
            maximum_duration,
            deadline_requirement,
            clock,
        )?);
        let protocol_rule = Arc::new(SupportedProtocolVersionsRule::new([
            panel_contracts::PROTOCOL_VERSION,
        ])?);
        Ok(Self {
            inner: CompositeGatewayRequestPolicy::new(budget_policy)
                .with_rule(Arc::new(RequiredRequestIdentityRule))
                .with_rule(Arc::new(BoundedGatewayRequestMetadataRule::new(
                    metadata_limits,
                )))
                .with_rule(protocol_rule)
                .with_rule(Arc::new(MutationIdempotencyRule)),
        })
    }
}

impl Default for StandardGatewayRequestPolicy {
    fn default() -> Self {
        Self::new(DEFAULT_REQUEST_TIMEOUT, DeadlineRequirement::Optional)
            .expect("default request policy is valid")
    }
}

impl GatewayRequestPolicy for StandardGatewayRequestPolicy {
    fn validate(
        &self,
        context: Option<&common::RequestContext>,
        class: RequestClass,
    ) -> Result<RequestExecutionBudget> {
        self.inner.validate(context, class)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedClock(SystemTime);

    impl DeadlineClock for FixedClock {
        fn now(&self) -> SystemTime {
            self.0
        }
    }

    fn context(deadline: &str) -> common::RequestContext {
        common::RequestContext {
            request_id: "request-1".into(),
            correlation_id: String::new(),
            actor: "test".into(),
            deadline: deadline.into(),
            idempotency_key: "mutation-1".into(),
            schema_version: panel_contracts::PROTOCOL_VERSION.into(),
        }
    }

    #[test]
    fn explicit_deadline_is_validated_and_caps_the_execution_budget() {
        let policy = StandardGatewayRequestPolicy::with_clock(
            Duration::from_secs(30),
            DeadlineRequirement::Optional,
            Arc::new(FixedClock(UNIX_EPOCH + Duration::from_secs(1))),
        )
        .unwrap();

        let valid = policy
            .validate(
                Some(&context("1970-01-01T00:00:10Z")),
                RequestClass::Mutation,
            )
            .unwrap();
        assert!(valid.timeout() <= Duration::from_secs(30));

        let expired = policy
            .validate(
                Some(&context("1970-01-01T00:00:01Z")),
                RequestClass::Mutation,
            )
            .unwrap_err();
        assert_eq!(
            expired.code.as_str(),
            panel_errors::ErrorCode::DEADLINE_EXCEEDED
        );
    }

    #[tokio::test]
    async fn execution_budget_cancels_work_that_exceeds_its_limit() {
        let budget = RequestExecutionBudget::new(Duration::from_millis(1)).unwrap();
        let error = budget
            .execute(std::future::pending::<Result<()>>())
            .await
            .unwrap_err();
        assert_eq!(
            error.code.as_str(),
            panel_errors::ErrorCode::DEADLINE_EXCEEDED
        );
    }

    struct RejectActorRule;

    impl GatewayRequestRule for RejectActorRule {
        fn validate(&self, request: RequestPolicyContext<'_>) -> Result<()> {
            if request.request_context().actor == "blocked" {
                return Err(PanelError::precondition_failed("actor is blocked"));
            }
            Ok(())
        }
    }

    #[test]
    fn composite_policy_accepts_extension_rules_without_facade_changes() {
        let policy = CompositeGatewayRequestPolicy::new(Arc::new(
            DeadlineRequestBudgetPolicy::new(Duration::from_secs(1), DeadlineRequirement::Optional)
                .unwrap(),
        ))
        .with_rule(Arc::new(RejectActorRule));
        let mut blocked = context("");
        blocked.actor = "blocked".into();

        let error = policy
            .validate(Some(&blocked), RequestClass::ReadOnly)
            .unwrap_err();
        assert_eq!(
            error.code.as_str(),
            panel_errors::ErrorCode::PRECONDITION_FAILED
        );
    }

    #[test]
    fn standard_policy_bounds_metadata_before_it_is_copied_into_events() {
        let policy = StandardGatewayRequestPolicy::default();
        let mut oversized = context("");
        oversized.request_id = "x".repeat(DEFAULT_MAX_REQUEST_ID_BYTES + 1);

        let error = policy
            .validate(Some(&oversized), RequestClass::ReadOnly)
            .unwrap_err();

        assert_eq!(
            error.code.as_str(),
            panel_errors::ErrorCode::RESOURCE_EXHAUSTED
        );
    }

    #[test]
    fn metadata_rule_rejects_control_characters_and_accepts_custom_limits() {
        let limits = GatewayRequestMetadataLimits::new(16, 16, 16, 32, 16, 16).unwrap();
        let rule = BoundedGatewayRequestMetadataRule::new(limits);
        let mut invalid = context("");
        invalid.correlation_id = "line\nbreak".into();

        let error = rule
            .validate(RequestPolicyContext {
                context: &invalid,
                class: RequestClass::ReadOnly,
            })
            .unwrap_err();

        assert_eq!(
            error.code.as_str(),
            panel_errors::ErrorCode::INVALID_ARGUMENT
        );
        assert_eq!(rule.limits(), limits);
        assert!(GatewayRequestMetadataLimits::new(0, 1, 1, 1, 1, 1).is_err());
    }

    #[test]
    fn standard_policy_accepts_injected_metadata_limits() {
        let limits = GatewayRequestMetadataLimits::new(4, 64, 64, 64, 64, 64).unwrap();
        let policy = StandardGatewayRequestPolicy::with_metadata_limits(
            Duration::from_secs(1),
            DeadlineRequirement::Optional,
            limits,
        )
        .unwrap();
        let mut oversized = context("");
        oversized.request_id = "12345".into();

        let error = policy
            .validate(Some(&oversized), RequestClass::ReadOnly)
            .unwrap_err();

        assert_eq!(
            error.code.as_str(),
            panel_errors::ErrorCode::RESOURCE_EXHAUSTED
        );
    }
}
