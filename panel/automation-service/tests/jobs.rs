#![forbid(unsafe_code)]

use automation_service::{PgJobStore, MIGRATIONS};
use chrono::{Duration as Span, Utc};
use panel_context::{IdempotencyKey, RequestId};
use panel_errors::{PanelError, Result};
use panel_jobs::{
    CancelOutcome, ClaimRequest, Finish, JobContext, JobError, JobHandler, JobId, JobKind,
    JobOrigin, JobSpec, JobState, JobStore, JobTemplate, MaintenanceWindow, Recurrence, Renewal,
    Schedule, ScheduleName, ScheduleStore, Worker, WorkerOptions,
};
use panel_platform::ServiceName;
use panel_postgres::{testing::TestDatabase, ServiceDatabase};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

fn kind() -> JobKind {
    JobKind::new("test.job").unwrap()
}

fn spec(key: &str) -> JobSpec {
    let request = RequestId::new(format!("request-{key}")).unwrap();
    JobSpec::json(
        kind(),
        IdempotencyKey::new(key).unwrap(),
        &serde_json::json!({ "key": key }),
        JobOrigin {
            correlation_id: RequestId::new("flow-1").unwrap(),
            causation_id: request,
        },
    )
    .unwrap()
}

async fn store() -> Option<(TestDatabase, ServiceDatabase, Arc<PgJobStore>)> {
    let mut database = TestDatabase::create().await?;
    let secrets = database.bootstrap(&[("automation", "automation")]).await;
    let service = database
        .connect_service("automation", "automation", &secrets[0])
        .await;
    service.migrate(MIGRATIONS).await.unwrap();
    let store = Arc::new(PgJobStore::new(
        &service,
        ServiceName::new("automation-service").unwrap(),
    ));
    Some((database, service, store))
}

async fn events(service: &ServiceDatabase) -> Vec<String> {
    let types: Vec<String> = sqlx::query_scalar("SELECT event_type FROM outbox ORDER BY position")
        .fetch_all(service.pool())
        .await
        .unwrap();
    types
        .into_iter()
        .map(|kind| {
            kind.trim_start_matches("io.github.eltavine.pingora-panel.automation.job.")
                .trim_end_matches(".v1")
                .to_owned()
        })
        .collect()
}

fn claim<'a>(owner: &'a str, kinds: &'a [JobKind], lease: Duration) -> ClaimRequest<'a> {
    ClaimRequest {
        owner,
        kinds,
        limit: 10,
        lease,
        open_windows: &[],
    }
}

#[tokio::test]
async fn leases_progress_and_outcomes_are_fenced_and_published() {
    let Some((database, service, store)) = store().await else {
        return;
    };
    let enqueued = store.enqueue(&spec("a")).await.unwrap();
    assert!(!store.enqueue(&spec("a")).await.unwrap().created);
    let kinds = [kind()];
    let lease = store
        .claim(&claim("worker-1", &kinds, Duration::from_secs(30)))
        .await
        .unwrap()
        .remove(0);
    assert_eq!((lease.attempt, lease.job.state), (1, JobState::Running));
    assert!(store
        .claim(&claim("worker-2", &kinds, Duration::from_secs(30)))
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store.renew(&lease, Duration::from_secs(30)).await.unwrap(),
        Renewal::Held {
            cancel_requested: false
        }
    );
    let progress = panel_jobs::Progress::new(40, "copying").unwrap();
    assert!(store.report_progress(&lease, &progress).await.unwrap());
    let stale = panel_jobs::Lease {
        owner: "impostor".into(),
        ..lease.clone()
    };
    assert!(!store.finish(&stale, Finish::Succeeded).await.unwrap());
    assert!(store.finish(&lease, Finish::Succeeded).await.unwrap());
    assert!(!store.finish(&lease, Finish::Succeeded).await.unwrap());

    let job = store.get(enqueued.job_id).await.unwrap().unwrap();
    assert_eq!(job.state, JobState::Succeeded);
    assert_eq!(job.progress, Some(progress));
    assert!(job.finished_at.is_some());
    assert_eq!(
        events(&service).await,
        ["queued", "started", "progressed", "succeeded"]
    );

    service.close().await;
    database.drop().await;
}

#[tokio::test]
async fn expired_leases_are_retried_cancelled_or_failed() {
    let Some((database, service, store)) = store().await else {
        return;
    };
    let kinds = [kind()];
    let short = Duration::from_millis(50);
    let retried = store.enqueue(&spec("retried")).await.unwrap();
    let first = store
        .claim(&claim("crashed", &kinds, short))
        .await
        .unwrap()
        .remove(0);
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(store.renew(&first, short).await.unwrap(), Renewal::Lost);
    let second = store
        .claim(&claim("survivor", &kinds, Duration::from_secs(30)))
        .await
        .unwrap()
        .remove(0);
    assert_eq!((second.job.id, second.attempt), (retried.job_id, 2));
    assert!(!store.finish(&first, Finish::Succeeded).await.unwrap());
    assert!(store
        .finish(
            &second,
            Finish::Retry {
                at: Utc::now() + Span::hours(1),
                error: JobError {
                    code: "UNAVAILABLE".into(),
                    message: "later".into()
                },
            },
        )
        .await
        .unwrap());
    assert_eq!(
        store.get(retried.job_id).await.unwrap().unwrap().state,
        JobState::Retrying
    );

    let cancelled = store.enqueue(&spec("cancelled")).await.unwrap();
    store.claim(&claim("crashed", &kinds, short)).await.unwrap();
    assert_eq!(
        store.cancel(cancelled.job_id).await.unwrap(),
        CancelOutcome::Requested
    );
    let exhausted = store
        .enqueue(&JobSpec {
            max_attempts: 1,
            ..spec("exhausted")
        })
        .await
        .unwrap();
    store.claim(&claim("crashed", &kinds, short)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(store
        .claim(&claim("survivor", &kinds, Duration::from_secs(30)))
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store.get(cancelled.job_id).await.unwrap().unwrap().state,
        JobState::Cancelled
    );
    let failed = store.get(exhausted.job_id).await.unwrap().unwrap();
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(failed.last_error.unwrap().code, "DEADLINE_EXCEEDED");

    let queued = store.enqueue(&spec("queued")).await.unwrap();
    assert_eq!(
        store.cancel(queued.job_id).await.unwrap(),
        CancelOutcome::Cancelled
    );
    assert_eq!(
        store.cancel(queued.job_id).await.unwrap(),
        CancelOutcome::AlreadyFinished(JobState::Cancelled)
    );
    assert_eq!(
        store
            .cancel(JobId::generate())
            .await
            .unwrap_err()
            .code
            .as_str(),
        panel_errors::ErrorCode::NOT_FOUND
    );

    service.close().await;
    database.drop().await;
}

#[tokio::test]
async fn concurrent_claims_never_share_a_job() {
    let Some((database, service, store)) = store().await else {
        return;
    };
    for index in 0..40 {
        store.enqueue(&spec(&format!("job-{index}"))).await.unwrap();
    }
    let mut claimers = Vec::new();
    for worker in 0..4 {
        let store = Arc::clone(&store);
        claimers.push(tokio::spawn(async move {
            let kinds = [kind()];
            let owner = format!("worker-{worker}");
            let mut claimed = Vec::new();
            for _ in 0..5 {
                for lease in store
                    .claim(&ClaimRequest {
                        limit: 3,
                        ..claim(&owner, &kinds, Duration::from_secs(30))
                    })
                    .await
                    .unwrap()
                {
                    claimed.push(lease.job.id);
                }
            }
            claimed
        }));
    }
    let mut all = Vec::new();
    for claimer in claimers {
        all.extend(claimer.await.unwrap());
    }
    let distinct: BTreeSet<_> = all.iter().collect();
    assert_eq!(all.len(), 40);
    assert_eq!(distinct.len(), 40);

    service.close().await;
    database.drop().await;
}

#[tokio::test]
async fn schedules_fire_once_per_occurrence_and_windows_gate_claims() {
    let Some((database, service, store)) = store().await else {
        return;
    };
    let start = Utc::now() - Span::minutes(5);
    let schedule = Schedule {
        name: ScheduleName::new("every-minute").unwrap(),
        recurrence: Recurrence::parse(format!(
            "DTSTART:{}\nRRULE:FREQ=MINUTELY",
            start.format("%Y%m%dT%H%M%SZ")
        ))
        .unwrap(),
        template: JobTemplate {
            kind: kind(),
            media_type: "application/json".into(),
            payload: b"{}".to_vec(),
            max_attempts: 1,
            priority: 0,
            maintenance_window: Some(ScheduleName::new("nightly").unwrap()),
        },
        enabled: true,
    };
    store
        .save_schedule(&schedule, start - Span::seconds(1))
        .await
        .unwrap();
    let now = Utc::now();
    let (first, second) = tokio::join!(store.fire_due(now), store.fire_due(now));
    assert_eq!(
        first.unwrap() + second.unwrap(),
        1,
        "one job per occurrence"
    );
    assert_eq!(store.fire_due(now).await.unwrap(), 0);

    let kinds = [kind()];
    assert!(
        store
            .claim(&claim("worker", &kinds, Duration::from_secs(30)))
            .await
            .unwrap()
            .is_empty(),
        "the job waits for its window"
    );
    store
        .save_window(
            &MaintenanceWindow::new(
                ScheduleName::new("nightly").unwrap(),
                Recurrence::parse(format!(
                    "DTSTART:{}\nRRULE:FREQ=DAILY",
                    (Utc::now() - Span::minutes(1)).format("%Y%m%dT%H%M%SZ")
                ))
                .unwrap(),
                Duration::from_secs(3600),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let windows: Vec<ScheduleName> = store
        .windows()
        .await
        .unwrap()
        .into_iter()
        .filter(|window| window.is_open(Utc::now()))
        .map(|window| window.name().clone())
        .collect();
    let claimed = store
        .claim(&ClaimRequest {
            open_windows: &windows,
            ..claim("worker", &kinds, Duration::from_secs(30))
        })
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert!(claimed[0]
        .job
        .idempotency_key
        .as_str()
        .starts_with("schedule:every-minute:"));

    service.close().await;
    database.drop().await;
}

struct Reporting;

#[async_trait::async_trait]
impl JobHandler for Reporting {
    async fn run(&self, context: JobContext) -> Result<()> {
        context.report(50, "half way")?;
        if context.attempt() == 1 {
            return Err(PanelError::unavailable("first attempt fails"));
        }
        context.report(100, "done")
    }
}

#[tokio::test]
async fn a_worker_runs_jobs_from_postgresql() {
    let Some((database, service, store)) = store().await else {
        return;
    };
    let job = store.enqueue(&spec("worked")).await.unwrap();
    let shutdown = CancellationToken::new();
    let worker = Worker::new(
        Arc::clone(&store) as Arc<dyn JobStore>,
        WorkerOptions {
            poll_interval: Duration::from_millis(20),
            retry: panel_jobs::RetryPolicy::new(
                Duration::from_millis(20),
                Duration::from_millis(40),
            )
            .unwrap(),
            progress_interval: Duration::from_millis(20),
            ..WorkerOptions::new("worker-1")
        },
    )
    .with_handler(kind(), Arc::new(Reporting));
    let task = tokio::spawn(worker.run(shutdown.clone()));
    let mut finished = None;
    for _ in 0..200 {
        let current = store.get(job.job_id).await.unwrap().unwrap();
        if current.state.is_final() {
            finished = Some(current);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    shutdown.cancel();
    task.await.unwrap();
    let finished = finished.expect("the job finishes");
    assert_eq!(
        (finished.state, finished.attempts),
        (JobState::Succeeded, 2)
    );
    assert_eq!(finished.progress.unwrap().message(), "done");
    let published = events(&service).await;
    assert_eq!(published.first().map(String::as_str), Some("queued"));
    assert!(published.contains(&"retrying".to_owned()));
    assert_eq!(published.last().map(String::as_str), Some("succeeded"));

    service.close().await;
    database.drop().await;
}
