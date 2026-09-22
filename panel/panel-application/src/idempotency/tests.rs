use super::*;
use crate::{IdempotencyKey, RequestDeadline, RequestId};
use panel_domain::RevisionId;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

struct FakeUseCases {
    activations: AtomicUsize,
}

#[async_trait]
impl GatewayUseCases for FakeUseCases {
    async fn validate(&self, _document: ConfigDocument) -> Result<ValidationReport> {
        Ok(ValidationReport::valid())
    }

    async fn prepare(
        &self,
        _context: CommandContext,
        _document: ConfigDocument,
    ) -> Result<PreparedDeployment> {
        Err(PanelError::internal("not used in idempotency test"))
    }

    async fn activate(
        &self,
        _context: CommandContext,
        _prepare_token: String,
        _expected_active_hash: Option<ContentHash>,
    ) -> Result<ActivatedDeployment> {
        self.activations.fetch_add(1, Ordering::SeqCst);
        Ok(ActivatedDeployment::new(
            RevisionId::new(1),
            ContentHash::from_bytes(b"active"),
            None,
        ))
    }
}

struct MemoryIdempotency {
    value: Mutex<Option<(IdempotencyKey, ContentHash, Option<IdempotencyRecord>)>>,
    fail_complete: bool,
}

#[async_trait]
impl IdempotencyRepository for MemoryIdempotency {
    async fn lookup(&self, key: &IdempotencyKey) -> Result<IdempotencyLookup> {
        Ok(match self.value.lock().unwrap().as_ref() {
            Some((stored_key, _, Some(record))) if stored_key == key => {
                IdempotencyLookup::Completed(record.clone())
            }
            Some((stored_key, _, None)) if stored_key == key => IdempotencyLookup::InProgress,
            _ => IdempotencyLookup::Missing,
        })
    }
    async fn claim(
        &self,
        key: &IdempotencyKey,
        request_hash: &ContentHash,
    ) -> Result<IdempotencyClaim> {
        let mut value = self.value.lock().unwrap();
        match value.as_ref() {
            None => {
                *value = Some((key.clone(), request_hash.clone(), None));
                Ok(IdempotencyClaim::Acquired)
            }
            Some((stored_key, stored_hash, record))
                if stored_key == key && stored_hash == request_hash =>
            {
                Ok(record
                    .clone()
                    .map_or(IdempotencyClaim::InProgress, IdempotencyClaim::Replay))
            }
            Some((stored_key, _, _)) if stored_key == key => Ok(IdempotencyClaim::Conflict),
            Some(_) => Ok(IdempotencyClaim::Conflict),
        }
    }

    async fn complete(&self, key: &IdempotencyKey, record: IdempotencyRecord) -> Result<()> {
        if self.fail_complete {
            return Err(PanelError::storage_unavailable(
                "test receipt store unavailable",
            ));
        }
        let mut value = self.value.lock().unwrap();
        if let Some((stored_key, _, stored_record)) = value.as_mut() {
            if stored_key == key {
                *stored_record = Some(record);
                return Ok(());
            }
        }
        Err(PanelError::internal("missing idempotency claim"))
    }

    async fn abort(&self, key: &IdempotencyKey, _request_hash: &ContentHash) -> Result<()> {
        let mut value = self.value.lock().unwrap();
        if value
            .as_ref()
            .is_some_and(|(stored_key, _, record)| stored_key == key && record.is_none())
        {
            *value = None;
        }
        Ok(())
    }
}

fn context(key: &str) -> CommandContext {
    CommandContext::new(
        RequestId::new("req-1").unwrap(),
        RequestId::new("corr-1").unwrap(),
        "tester",
        RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
        IdempotencyKey::new(key).unwrap(),
    )
    .unwrap()
}

struct RejectedActivation {
    code: &'static str,
    activations: AtomicUsize,
}

#[async_trait]
impl GatewayUseCases for RejectedActivation {
    async fn validate(&self, _: ConfigDocument) -> Result<ValidationReport> {
        unreachable!()
    }
    async fn prepare(&self, _: CommandContext, _: ConfigDocument) -> Result<PreparedDeployment> {
        unreachable!()
    }
    async fn activate(
        &self,
        _: CommandContext,
        _: String,
        _: Option<ContentHash>,
    ) -> Result<ActivatedDeployment> {
        self.activations.fetch_add(1, Ordering::SeqCst);
        // Retryability deliberately does not establish commit certainty.
        Err(PanelError::new(self.code, "activation failed").retryable(true))
    }
}

#[tokio::test]
async fn uncertain_and_future_errors_retain_claim_and_block_redispatch() {
    use panel_errors::ErrorCode;
    for code in [
        ErrorCode::COMMIT_OUTCOME_UNKNOWN,
        ErrorCode::DEADLINE_EXCEEDED,
        ErrorCode::STORAGE_UNAVAILABLE,
        ErrorCode::INTERNAL,
        ErrorCode::ACTIVATE_FAILED,
        ErrorCode::RESOURCE_EXHAUSTED,
        ErrorCode::CORRUPT_STATE,
        "FUTURE_ERROR",
    ] {
        let gateway = Arc::new(RejectedActivation {
            code,
            activations: AtomicUsize::new(0),
        });
        let repository = Arc::new(MemoryIdempotency {
            value: Mutex::new(None),
            fail_complete: false,
        });
        let service = IdempotentGatewayUseCases::new(gateway.clone(), repository.clone());
        let error = service
            .activate(context("uncertain"), "prepare-1".into(), None)
            .await
            .unwrap_err();
        assert_eq!(error.code.as_str(), code);
        assert_eq!(
            service
                .activation_receipt(context("uncertain").idempotency_key())
                .await
                .unwrap(),
            IdempotencyLookup::InProgress
        );
        let error = service
            .activate(context("uncertain"), "prepare-1".into(), None)
            .await
            .unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::RESOURCE_EXHAUSTED);
        let error = service
            .activate(context("uncertain"), "prepare-2".into(), None)
            .await
            .unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::CONFLICT);
        assert_eq!(gateway.activations.load(Ordering::SeqCst), 1, "{code}");
    }
}

#[tokio::test]
async fn confirmed_precommit_rejections_release_claim_for_another_attempt() {
    use panel_errors::ErrorCode;
    for code in [
        ErrorCode::INVALID_ARGUMENT,
        ErrorCode::VALIDATION_FAILED,
        ErrorCode::CONFLICT,
        ErrorCode::NOT_FOUND,
        ErrorCode::PRECONDITION_FAILED,
        ErrorCode::UNSUPPORTED_CAPABILITY,
        ErrorCode::UNAUTHENTICATED,
        ErrorCode::PERMISSION_DENIED,
    ] {
        let gateway = Arc::new(RejectedActivation {
            code,
            activations: AtomicUsize::new(0),
        });
        let repository = Arc::new(MemoryIdempotency {
            value: Mutex::new(None),
            fail_complete: false,
        });
        let service = IdempotentGatewayUseCases::new(gateway.clone(), repository.clone());
        for _ in 0..2 {
            assert_eq!(
                service
                    .activate(context("rejected"), "prepare-1".into(), None)
                    .await
                    .unwrap_err()
                    .code
                    .as_str(),
                code
            );
            assert_eq!(
                service
                    .activation_receipt(context("rejected").idempotency_key())
                    .await
                    .unwrap(),
                IdempotencyLookup::Missing
            );
        }
        assert_eq!(gateway.activations.load(Ordering::SeqCst), 2);
    }
}

#[test]
fn activation_request_hash_preserves_the_legacy_receipt_encoding() {
    let expected_hash = ContentHash::from_bytes(b"active");
    assert_eq!(
        activation_request_hash("prepare-1", Some(&expected_hash)),
        ContentHash::from_bytes(format!("prepare-1\0{expected_hash}").as_bytes())
    );
    assert_eq!(
        activation_request_hash("prepare-1", None),
        ContentHash::from_bytes(b"prepare-1\0")
    );
}

#[tokio::test]
async fn activation_replays_receipt_and_rejects_hash_reuse() {
    let gateway = Arc::new(FakeUseCases {
        activations: AtomicUsize::new(0),
    });
    let repository = Arc::new(MemoryIdempotency {
        value: Mutex::new(None),
        fail_complete: false,
    });
    let service = IdempotentGatewayUseCases::new(gateway.clone(), repository);

    let first = service
        .activate(context("idem-1"), "prepare-1".into(), None)
        .await
        .unwrap();
    let second = service
        .activate(context("idem-1"), "prepare-1".into(), None)
        .await
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(gateway.activations.load(Ordering::SeqCst), 1);

    let conflict = service
        .activate(context("idem-1"), "prepare-2".into(), None)
        .await
        .unwrap_err();
    assert_eq!(conflict.code.as_str(), panel_errors::ErrorCode::CONFLICT);
}

#[tokio::test]
async fn atomic_claim_blocks_a_concurrent_duplicate_before_gateway_execution() {
    let repository = MemoryIdempotency {
        value: Mutex::new(None),
        fail_complete: false,
    };
    let key = IdempotencyKey::new("idem-claim").unwrap();
    let hash = ContentHash::from_bytes(b"request");
    assert_eq!(
        repository.claim(&key, &hash).await.unwrap(),
        IdempotencyClaim::Acquired
    );
    assert_eq!(
        repository.claim(&key, &hash).await.unwrap(),
        IdempotencyClaim::InProgress
    );

    let deployment =
        ActivatedDeployment::new(RevisionId::new(1), ContentHash::from_bytes(b"active"), None);
    repository
        .complete(
            &key,
            IdempotencyRecord::new(hash.clone(), DeploymentOutcome::Succeeded(deployment)),
        )
        .await
        .unwrap();
    assert!(matches!(
        repository.claim(&key, &hash).await.unwrap(),
        IdempotencyClaim::Replay(_)
    ));
}

#[tokio::test]
async fn receipt_failure_never_releases_claim_after_gateway_success() {
    let gateway = Arc::new(FakeUseCases {
        activations: AtomicUsize::new(0),
    });
    let repository = Arc::new(MemoryIdempotency {
        value: Mutex::new(None),
        fail_complete: true,
    });
    let service = IdempotentGatewayUseCases::new(gateway.clone(), repository);

    let error = service
        .activate(context("idem-receipt-failure"), "prepare-1".into(), None)
        .await
        .unwrap_err();
    assert_eq!(
        error.code.as_str(),
        panel_errors::ErrorCode::COMMIT_OUTCOME_UNKNOWN
    );
    assert!(error.retryable);
    assert_eq!(gateway.activations.load(Ordering::SeqCst), 1);
}
