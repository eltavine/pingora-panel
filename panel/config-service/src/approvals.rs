//! Approvals of configuration changes (ADR 0019): the policies, the requests
//! applying covered changes opens, and the approvals that let them through.

use chrono::{DateTime, Duration, Utc};
use panel_config_dsl::plan::plan;
use panel_config_model::{
    ApprovalDecision, ApprovalPolicy, ApprovalPolicyInput, ApprovalRequest, ApprovalState,
    Assessment, ConfigModel, PlannedChange, PolicyVersion, Risk, REQUEST_LIFETIME,
};
use panel_errors::{PanelError, Result};
use panel_event_contracts::config::v1 as event;
use panel_events::EventData;
use panel_events::RequestScope;
use panel_postgres::{storage_error, EventLog, PgOutbox, ServiceDatabase};
use serde::Serialize;
use sqlx::{postgres::PgRow, PgPool, Postgres, Row, Transaction};
use std::collections::BTreeSet;
use uuid::Uuid;

macro_rules! select_requests {
    ($rest:literal) => {
        concat!(
            "SELECT id, state, draft_version, content_hash, requested_by, requested_at, ",
            "expires_at, note, risk, policies::text AS policies, required, valid_minutes, ",
            "changes::text AS changes, closed_by, closed_at, reason, revision ",
            "FROM approval_requests ",
            $rest
        )
    };
}

/// What a change touches, from the plan people review.
pub(crate) fn assess(current: &ConfigModel, next: &ConfigModel) -> Assessment {
    let changes: Vec<PlannedChange> = plan(current, next)
        .into_iter()
        .map(|change| PlannedChange {
            resource: change.resource,
            change: serde_json::to_value(change.change)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default(),
        })
        .collect();
    let touched: BTreeSet<&str> = changes
        .iter()
        .filter_map(|change| change.resource.strip_prefix("sites/"))
        .collect();
    let site_tags = [current, next]
        .iter()
        .flat_map(|model| model.sites.iter())
        .filter(|site| touched.contains(site.id.to_string().as_str()))
        .flat_map(|site| site.tags.iter().cloned())
        .collect();
    Assessment::new(changes, site_tags)
}

/// What the approvals say about applying a change.
pub(crate) enum Gate {
    /// No policy covers it.
    Clear,
    /// Enough people approved exactly this content under these policies.
    Approved(Uuid),
    /// It waits for approvals.
    Awaiting(Box<ApprovalRequest>),
}

/// A change to open a request for.
pub(crate) struct Opening<'a> {
    pub draft_version: u64,
    pub content_hash: &'a str,
    pub note: Option<&'a str>,
    pub assessment: Assessment,
    pub covering: Vec<&'a ApprovalPolicy>,
}

/// Why an Administrator applied without the approvals.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Bypass {
    pub reason: String,
    pub incident: String,
}

fn corrupt(what: &str) -> impl Fn(sqlx::Error) -> PanelError + '_ {
    move |error| PanelError::corrupt_state(format!("stored {what} is invalid: {error}"))
}

fn json<T: for<'de> serde::Deserialize<'de>>(text: &str, what: &str) -> Result<T> {
    serde_json::from_str(text)
        .map_err(|error| PanelError::corrupt_state(format!("stored {what} is invalid: {error}")))
}

fn unsigned(value: i64, what: &str) -> Result<u64> {
    u64::try_from(value)
        .map_err(|_| PanelError::corrupt_state(format!("stored {what} is negative")))
}

fn policy(row: &PgRow) -> Result<ApprovalPolicy> {
    let column = corrupt("approval policy");
    let body: String = row.try_get("policy").map_err(&column)?;
    Ok(ApprovalPolicy {
        id: row.try_get("id").map_err(&column)?,
        policy: json(&body, "approval policy")?,
        version: unsigned(row.try_get("version").map_err(&column)?, "policy version")?,
        created_at: row.try_get("created_at").map_err(&column)?,
        updated_at: row.try_get("updated_at").map_err(&column)?,
    })
}

fn stored_state(value: &str) -> Result<ApprovalState> {
    Ok(match value {
        "pending" => ApprovalState::Pending,
        "approved" => ApprovalState::Approved,
        "rejected" => ApprovalState::Rejected,
        "withdrawn" => ApprovalState::Withdrawn,
        "outdated" => ApprovalState::Outdated,
        "applied" => ApprovalState::Applied,
        other => {
            return Err(PanelError::corrupt_state(format!(
                "stored approval state {other:?} is unknown"
            )))
        }
    })
}

fn request(row: &PgRow, approvals: Vec<ApprovalDecision>) -> Result<ApprovalRequest> {
    let column = corrupt("approval request");
    let state: String = row.try_get("state").map_err(&column)?;
    let risk: String = row.try_get("risk").map_err(&column)?;
    let policies: String = row.try_get("policies").map_err(&column)?;
    let changes: String = row.try_get("changes").map_err(&column)?;
    let revision: Option<i64> = row.try_get("revision").map_err(&column)?;
    Ok(ApprovalRequest {
        id: row.try_get("id").map_err(&column)?,
        state: stored_state(&state)?,
        draft_version: unsigned(
            row.try_get("draft_version").map_err(&column)?,
            "draft version",
        )?,
        content_hash: row.try_get("content_hash").map_err(&column)?,
        requested_by: row.try_get("requested_by").map_err(&column)?,
        requested_at: row.try_get("requested_at").map_err(&column)?,
        expires_at: row.try_get("expires_at").map_err(&column)?,
        note: row.try_get("note").map_err(&column)?,
        risk: match risk.as_str() {
            "high" => Risk::High,
            "low" => Risk::Low,
            other => {
                return Err(PanelError::corrupt_state(format!(
                    "stored risk {other:?} is unknown"
                )))
            }
        },
        policies: json(&policies, "policies")?,
        required: u32::try_from(row.try_get::<i32, _>("required").map_err(&column)?)
            .map_err(|_| PanelError::corrupt_state("stored approval count is negative"))?,
        valid_minutes: u32::try_from(row.try_get::<i32, _>("valid_minutes").map_err(&column)?)
            .map_err(|_| PanelError::corrupt_state("stored validity is negative"))?,
        changes: json(&changes, "changes")?,
        approvals,
        closed_by: row.try_get("closed_by").map_err(&column)?,
        closed_at: row.try_get("closed_at").map_err(&column)?,
        reason: row.try_get("reason").map_err(&column)?,
        revision: revision
            .map(|value| unsigned(value, "revision"))
            .transpose()?,
    })
}

fn decision(row: &PgRow) -> Result<ApprovalDecision> {
    let column = corrupt("approval");
    Ok(ApprovalDecision {
        approver: row.try_get("approver").map_err(&column)?,
        approved_at: row.try_get("approved_at").map_err(&column)?,
        valid_until: row.try_get("valid_until").map_err(&column)?,
        revoked_at: row.try_get("revoked_at").map_err(&column)?,
    })
}

fn risk_text(risk: Risk) -> &'static str {
    match risk {
        Risk::High => "high",
        _ => "low",
    }
}

/// Policies, requests and approvals in the configuration schema.
#[derive(Clone)]
pub struct PgApprovals {
    pool: PgPool,
    events: EventLog,
}

/// How an open request closes without being applied.
#[derive(Clone, Copy)]
enum Closing {
    Rejected,
    Withdrawn,
    Outdated,
}

impl Closing {
    fn state(self) -> &'static str {
        match self {
            Self::Rejected => "rejected",
            Self::Withdrawn => "withdrawn",
            Self::Outdated => "outdated",
        }
    }
}

/// The event recording a stored policy, created or updated alike.
macro_rules! policy_event {
    ($event:path, $stored:expr) => {{
        let stored: &ApprovalPolicy = $stored;
        let policy = &stored.policy;
        $event {
            id: stored.id.clone(),
            version: stored.version,
            description: policy.description.clone(),
            resources: policy.resources.clone(),
            site_tags: policy.site_tags.clone(),
            min_risk: policy.min_risk.as_str().into(),
            windows: policy
                .windows
                .iter()
                .map(|window| event::Window {
                    recurrence: window.recurrence().as_str().to_owned(),
                    minutes: u32::try_from(window.duration().as_secs() / 60).unwrap_or(u32::MAX),
                })
                .collect(),
            approvals: policy.approvals,
            valid_minutes: policy.valid_minutes,
            enabled: policy.enabled,
        }
    }};
}

impl PgApprovals {
    pub fn new(database: &ServiceDatabase, events: EventLog) -> Self {
        Self {
            pool: database.pool().clone(),
            events,
        }
    }

    async fn emit<E: EventData>(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        aggregate: (&str, &str),
        scope: &RequestScope,
        actor: &str,
        data: &E,
    ) -> Result<()> {
        let event = self.events.event(aggregate, scope, actor, data)?;
        PgOutbox::append(transaction, &event).await
    }

    pub async fn policies(&self) -> Result<Vec<ApprovalPolicy>> {
        sqlx::query(
            "SELECT id, policy::text AS policy, version, created_at, updated_at \
             FROM approval_policies ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?
        .iter()
        .map(policy)
        .collect()
    }

    pub async fn policy(&self, id: &str) -> Result<ApprovalPolicy> {
        sqlx::query(
            "SELECT id, policy::text AS policy, version, created_at, updated_at \
             FROM approval_policies WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?
        .map(|row| policy(&row))
        .transpose()?
        .ok_or_else(|| PanelError::not_found(format!("there is no approval policy {id}")))
    }

    /// Creates or replaces a policy; true when it is new.
    pub async fn put_policy(
        &self,
        id: &str,
        input: ApprovalPolicyInput,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<(ApprovalPolicy, bool)> {
        let problems = input.problems(id);
        if !problems.is_empty() {
            return Err(PanelError::validation_failed(problems.join("; ")));
        }
        let body = serde_json::to_string(&input).expect("policies serialize");
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let row = sqlx::query(
            "INSERT INTO approval_policies (id, policy, version, created_at, updated_at) \
             VALUES ($1, $2::jsonb, 1, now(), now()) \
             ON CONFLICT (id) DO UPDATE SET policy = EXCLUDED.policy, \
             version = approval_policies.version + 1, updated_at = now() \
             RETURNING id, policy::text AS policy, version, created_at, updated_at, \
             (xmax = 0) AS created",
        )
        .bind(id)
        .bind(&body)
        .fetch_one(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let created: bool = row.try_get("created").map_err(storage_error)?;
        let stored = policy(&row)?;
        if created {
            self.emit(
                &mut transaction,
                ("approval_policy", id),
                scope,
                actor,
                &policy_event!(event::ApprovalPolicyCreated, &stored),
            )
            .await?;
        } else {
            self.emit(
                &mut transaction,
                ("approval_policy", id),
                scope,
                actor,
                &policy_event!(event::ApprovalPolicyUpdated, &stored),
            )
            .await?;
        }
        transaction.commit().await.map_err(storage_error)?;
        Ok((stored, created))
    }

    pub async fn delete_policy(&self, id: &str, scope: &RequestScope, actor: &str) -> Result<()> {
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let deleted = sqlx::query("DELETE FROM approval_policies WHERE id = $1")
            .bind(id)
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
        if deleted.rows_affected() == 0 {
            return Err(PanelError::not_found(format!(
                "there is no approval policy {id}"
            )));
        }
        self.emit(
            &mut transaction,
            ("approval_policy", id),
            scope,
            actor,
            &event::ApprovalPolicyDeleted {
                policy: id.to_owned(),
            },
        )
        .await?;
        transaction.commit().await.map_err(storage_error)
    }

    async fn approvals_of(&self, ids: &[Uuid]) -> Result<Vec<(Uuid, ApprovalDecision)>> {
        sqlx::query(
            "SELECT request_id, approver, approved_at, valid_until, revoked_at FROM approvals \
             WHERE request_id = ANY($1) ORDER BY approved_at",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?
        .iter()
        .map(|row| {
            Ok((
                row.try_get("request_id").map_err(storage_error)?,
                decision(row)?,
            ))
        })
        .collect()
    }

    async fn with_approvals(&self, rows: Vec<PgRow>) -> Result<Vec<ApprovalRequest>> {
        let ids: Vec<Uuid> = rows
            .iter()
            .map(|row| row.try_get("id").map_err(storage_error))
            .collect::<Result<_>>()?;
        let mut approvals = self.approvals_of(&ids).await?;
        rows.iter()
            .zip(ids)
            .map(|(row, id)| {
                let (mine, rest): (Vec<_>, Vec<_>) =
                    approvals.drain(..).partition(|(request, _)| *request == id);
                approvals = rest;
                request(
                    row,
                    mine.into_iter().map(|(_, approval)| approval).collect(),
                )
            })
            .collect()
    }

    /// Requests newest first, as stored.
    pub async fn requests(
        &self,
        before: Option<DateTime<Utc>>,
        limit: u32,
    ) -> Result<Vec<ApprovalRequest>> {
        let rows = sqlx::query(select_requests!(
            "WHERE ($1::timestamptz IS NULL OR requested_at < $1) \
             ORDER BY requested_at DESC LIMIT $2"
        ))
        .bind(before)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?;
        self.with_approvals(rows).await
    }

    pub async fn request(&self, id: Uuid) -> Result<ApprovalRequest> {
        let row = sqlx::query(select_requests!("WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .ok_or_else(|| PanelError::not_found(format!("there is no approval request {id}")))?;
        Ok(self.with_approvals(vec![row]).await?.remove(0))
    }

    /// Decides what applying `content_hash` needs, opening a request when
    /// covering policies have no valid one.
    pub(crate) async fn gate(
        &self,
        opening: Opening<'_>,
        now: DateTime<Utc>,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<Gate> {
        if opening.covering.is_empty() {
            return Ok(Gate::Clear);
        }
        let mut versions: Vec<PolicyVersion> = opening
            .covering
            .iter()
            .map(|policy| PolicyVersion {
                id: policy.id.clone(),
                version: policy.version,
            })
            .collect();
        versions.sort();
        let open = sqlx::query(select_requests!(
            "WHERE content_hash = $1 AND state IN ('pending', 'approved') \
             ORDER BY requested_at DESC"
        ))
        .bind(opening.content_hash)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?;
        for existing in self.with_approvals(open).await? {
            let current = existing.policies == versions;
            match existing.state_at(now, opening.content_hash) {
                ApprovalState::Approved if current => return Ok(Gate::Approved(existing.id)),
                ApprovalState::Pending if current => return Ok(Gate::Awaiting(Box::new(existing))),
                _ => {
                    self.close(existing.id, Closing::Outdated, actor, None, scope, now)
                        .await?;
                }
            }
        }
        let required = opening
            .covering
            .iter()
            .map(|policy| policy.policy.approvals)
            .max()
            .unwrap_or(1);
        let valid_minutes = opening
            .covering
            .iter()
            .map(|policy| policy.policy.valid_minutes)
            .min()
            .unwrap_or(60);
        let created = ApprovalRequest {
            id: Uuid::now_v7(),
            state: ApprovalState::Pending,
            draft_version: opening.draft_version,
            content_hash: opening.content_hash.to_owned(),
            requested_by: actor.to_owned(),
            requested_at: now,
            expires_at: now + REQUEST_LIFETIME,
            note: opening.note.map(str::to_owned),
            risk: opening.assessment.risk,
            policies: versions,
            required,
            valid_minutes,
            changes: opening.assessment.changes,
            approvals: Vec::new(),
            closed_by: None,
            closed_at: None,
            reason: None,
            revision: None,
        };
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query(
            "INSERT INTO approval_requests (id, state, draft_version, content_hash, \
             requested_by, requested_at, expires_at, note, risk, policies, required, \
             valid_minutes, changes) \
             VALUES ($1, 'pending', $2, $3, $4, $5, $6, $7, $8, $9::jsonb, $10, $11, $12::jsonb)",
        )
        .bind(created.id)
        .bind(i64::try_from(created.draft_version).unwrap_or(i64::MAX))
        .bind(&created.content_hash)
        .bind(&created.requested_by)
        .bind(created.requested_at)
        .bind(created.expires_at)
        .bind(&created.note)
        .bind(risk_text(created.risk))
        .bind(serde_json::to_string(&created.policies).expect("policies serialize"))
        .bind(i32::try_from(created.required).unwrap_or(i32::MAX))
        .bind(i32::try_from(created.valid_minutes).unwrap_or(i32::MAX))
        .bind(serde_json::to_string(&created.changes).expect("changes serialize"))
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        self.emit(
            &mut transaction,
            ("approval_request", &created.id.to_string()),
            scope,
            actor,
            &event::ApprovalRequested {
                request: created.id.to_string(),
                draft_version: created.draft_version,
                content_hash: created.content_hash.clone(),
                risk: created.risk.as_str().into(),
                policies: created
                    .policies
                    .iter()
                    .map(|policy| event::PolicyVersion {
                        id: policy.id.clone(),
                        version: policy.version,
                    })
                    .collect(),
                required: created.required,
                changes: created
                    .changes
                    .iter()
                    .map(|change| event::PlannedChange {
                        resource: change.resource.clone(),
                        change: change.change.clone(),
                    })
                    .collect(),
            },
        )
        .await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(Gate::Awaiting(Box::new(created)))
    }

    /// Locks an open request for a decision, with its state at `now`.
    async fn locked(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        id: Uuid,
    ) -> Result<ApprovalRequest> {
        let row = sqlx::query(select_requests!("WHERE id = $1 FOR UPDATE"))
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(storage_error)?
            .ok_or_else(|| PanelError::not_found(format!("there is no approval request {id}")))?;
        let approvals = sqlx::query(
            "SELECT approver, approved_at, valid_until, revoked_at FROM approvals \
             WHERE request_id = $1 ORDER BY approved_at",
        )
        .bind(id)
        .fetch_all(&mut **transaction)
        .await
        .map_err(storage_error)?
        .iter()
        .map(decision)
        .collect::<Result<_>>()?;
        request(&row, approvals)
    }

    fn open_at(request: &ApprovalRequest, now: DateTime<Utc>, draft_hash: &str) -> Result<()> {
        match request.state_at(now, draft_hash) {
            ApprovalState::Pending | ApprovalState::Approved => Ok(()),
            state => Err(PanelError::precondition_failed(format!(
                "the request is {}, so it can no longer be decided",
                serde_json::to_value(state)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_default()
            ))),
        }
    }

    /// Records `approver`'s approval of content the draft still has.
    pub async fn approve(
        &self,
        id: Uuid,
        approver: &str,
        draft_hash: &str,
        now: DateTime<Utc>,
        scope: &RequestScope,
    ) -> Result<ApprovalRequest> {
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let request = self.locked(&mut transaction, id).await?;
        Self::open_at(&request, now, draft_hash)?;
        if request.requested_by == approver {
            return Err(PanelError::permission_denied(
                "nobody may approve their own request",
            ));
        }
        if request
            .approvals
            .iter()
            .any(|approval| approval.approver == approver && approval.is_valid(now))
        {
            return Err(PanelError::conflict("you already approved this request"));
        }
        let valid_until = now + Duration::minutes(i64::from(request.valid_minutes));
        sqlx::query(
            "INSERT INTO approvals (request_id, approver, approved_at, valid_until) \
             VALUES ($1, $2, $3, $4) ON CONFLICT (request_id, approver) DO UPDATE SET \
             approved_at = EXCLUDED.approved_at, valid_until = EXCLUDED.valid_until, \
             revoked_at = NULL",
        )
        .bind(id)
        .bind(approver)
        .bind(now)
        .bind(valid_until)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let approved = request.valid_approvals(now) + 1 >= request.required as usize;
        if approved {
            sqlx::query("UPDATE approval_requests SET state = 'approved' WHERE id = $1")
                .bind(id)
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?;
        }
        self.emit(
            &mut transaction,
            ("approval_request", &id.to_string()),
            scope,
            approver,
            &event::ApprovalApproved {
                request: id.to_string(),
                valid_until: Some(valid_until.into()),
                approved,
            },
        )
        .await?;
        transaction.commit().await.map_err(storage_error)?;
        self.request(id).await
    }

    /// Rejects an open request someone else asked for.
    pub async fn reject(
        &self,
        id: Uuid,
        approver: &str,
        reason: Option<&str>,
        draft_hash: &str,
        now: DateTime<Utc>,
        scope: &RequestScope,
    ) -> Result<ApprovalRequest> {
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let request = self.locked(&mut transaction, id).await?;
        Self::open_at(&request, now, draft_hash)?;
        if request.requested_by == approver {
            return Err(PanelError::permission_denied(
                "withdraw your own request instead of rejecting it",
            ));
        }
        drop(transaction);
        self.close(id, Closing::Rejected, approver, reason, scope, now)
            .await?;
        self.request(id).await
    }

    /// Withdraws a request its requester no longer wants.
    pub async fn withdraw(
        &self,
        id: Uuid,
        actor: &str,
        now: DateTime<Utc>,
        scope: &RequestScope,
    ) -> Result<ApprovalRequest> {
        let request = self.request(id).await?;
        if request.requested_by != actor {
            return Err(PanelError::permission_denied(
                "only the person who asked can withdraw a request",
            ));
        }
        if !matches!(
            request.state,
            ApprovalState::Pending | ApprovalState::Approved
        ) {
            return Err(PanelError::precondition_failed(
                "the request is already closed",
            ));
        }
        self.close(id, Closing::Withdrawn, actor, None, scope, now)
            .await?;
        self.request(id).await
    }

    /// Takes back `approver`'s approval of a request not yet applied.
    pub async fn revoke(
        &self,
        id: Uuid,
        approver: &str,
        now: DateTime<Utc>,
        scope: &RequestScope,
    ) -> Result<ApprovalRequest> {
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let request = self.locked(&mut transaction, id).await?;
        if !matches!(
            request.state,
            ApprovalState::Pending | ApprovalState::Approved
        ) {
            return Err(PanelError::precondition_failed(
                "the request is already closed",
            ));
        }
        let revoked = sqlx::query(
            "UPDATE approvals SET revoked_at = $3 \
             WHERE request_id = $1 AND approver = $2 AND revoked_at IS NULL",
        )
        .bind(id)
        .bind(approver)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        if revoked.rows_affected() == 0 {
            return Err(PanelError::not_found("you have not approved this request"));
        }
        let remaining = request
            .approvals
            .iter()
            .filter(|approval| approval.approver != approver && approval.is_valid(now))
            .count();
        if remaining < request.required as usize {
            sqlx::query("UPDATE approval_requests SET state = 'pending' WHERE id = $1")
                .bind(id)
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?;
        }
        self.emit(
            &mut transaction,
            ("approval_request", &id.to_string()),
            scope,
            approver,
            &event::ApprovalRevoked {
                request: id.to_string(),
            },
        )
        .await?;
        transaction.commit().await.map_err(storage_error)?;
        self.request(id).await
    }

    /// Records that the approved request was applied as `revision`.
    pub async fn applied(
        &self,
        id: Uuid,
        revision: u64,
        actor: &str,
        scope: &RequestScope,
    ) -> Result<()> {
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query(
            "UPDATE approval_requests SET state = 'applied', revision = $2, closed_by = $3, \
             closed_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(i64::try_from(revision).unwrap_or(i64::MAX))
        .bind(actor)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        self.emit(
            &mut transaction,
            ("approval_request", &id.to_string()),
            scope,
            actor,
            &event::ApprovalApplied {
                request: id.to_string(),
                revision,
            },
        )
        .await?;
        transaction.commit().await.map_err(storage_error)
    }

    /// Records an emergency bypass before anything is published, closing
    /// open requests for the same content; nothing may be applied unless
    /// this is recorded.
    pub(crate) async fn bypassed(
        &self,
        bypass: &Bypass,
        content_hash: &str,
        covering: &[&ApprovalPolicy],
        scope: &RequestScope,
        actor: &str,
    ) -> Result<()> {
        if bypass.reason.trim().chars().count() < 10 || bypass.incident.trim().is_empty() {
            return Err(PanelError::invalid_argument(
                "an emergency bypass needs a reason of at least 10 characters and an incident",
            ));
        }
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query(
            "UPDATE approval_requests SET state = 'applied', closed_by = $2, closed_at = now(), \
             reason = $3 WHERE content_hash = $1 AND state IN ('pending', 'approved')",
        )
        .bind(content_hash)
        .bind(actor)
        .bind(format!("bypassed: {}", bypass.reason.trim()))
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        self.emit(
            &mut transaction,
            ("configuration", "draft"),
            scope,
            actor,
            &event::ApprovalBypassed {
                reason: bypass.reason.trim().to_owned(),
                incident: bypass.incident.trim().to_owned(),
                content_hash: content_hash.to_owned(),
                policies: covering.iter().map(|policy| policy.id.clone()).collect(),
            },
        )
        .await?;
        transaction.commit().await.map_err(storage_error)
    }

    async fn close(
        &self,
        id: Uuid,
        closing: Closing,
        actor: &str,
        reason: Option<&str>,
        scope: &RequestScope,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query(
            "UPDATE approval_requests SET state = $2, closed_by = $3, closed_at = $4, reason = $5 \
             WHERE id = $1",
        )
        .bind(id)
        .bind(closing.state())
        .bind(actor)
        .bind(now)
        .bind(reason)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let request = id.to_string();
        match closing {
            Closing::Rejected => {
                self.emit(
                    &mut transaction,
                    ("approval_request", &request),
                    scope,
                    actor,
                    &event::ApprovalRejected {
                        request: request.clone(),
                        reason: reason.map(str::to_owned),
                    },
                )
                .await?;
            }
            Closing::Withdrawn => {
                self.emit(
                    &mut transaction,
                    ("approval_request", &request),
                    scope,
                    actor,
                    &event::ApprovalWithdrawn {
                        request: request.clone(),
                    },
                )
                .await?;
            }
            Closing::Outdated => {
                self.emit(
                    &mut transaction,
                    ("approval_request", &request),
                    scope,
                    actor,
                    &event::ApprovalOutdated {
                        request: request.clone(),
                    },
                )
                .await?;
            }
        }
        transaction.commit().await.map_err(storage_error)
    }
}
