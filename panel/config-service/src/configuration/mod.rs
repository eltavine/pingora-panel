use crate::{
    language, operations, scope,
    store::{
        self, ApprovalStore, ChangeOutput, ChangeRequest, DraftChange, DraftState, DraftStore,
        EventRecorder, NewRevision, RevisionStore, DRAFT,
    },
};
use async_trait::async_trait;
use chrono::Utc;
use panel_application::{
    ActivatedDeployment, ApplyOutcome, ApplyRequest, CommandContext, ConfigDocument,
    ConfigurationChange, ConfigurationOutput, ConfigurationPort, ConfigurationRead, ContentHash,
    DraftInfo, GatewayUseCases, RequestScope,
};
use panel_config_dsl::{
    explain, format_files, import_nginx, plan::changes, schema::DIRECTIVES, syntax_tree, Sources,
    ENTRY, LANGUAGE_VERSION,
};
use panel_config_model::{
    compile, ApprovalRequest, ConfigModel, Revision, RevisionDetail, RevisionList,
};
use panel_domain::RevisionId;
use panel_engine::{validate_engine_ir, EngineCapability};
use panel_errors::{Diagnostic, DiagnosticSeverity, PanelError, Result, ValidationReport};
use panel_event_contracts::config::v1 as event;
use panel_ir::{RuntimeSnapshot, IR_SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};

mod approval_operations;

const DEFAULT_REVISION_PAGE: u32 = 50;
const MAX_REVISION_PAGE: u32 = 500;

/// The configuration use cases behind the configuration port: reads and
/// changes of the draft, its files and revisions, approvals, and applying the
/// draft to the gateway through the publication use cases.
pub struct ConfigurationService {
    drafts: Arc<dyn DraftStore>,
    revisions: Arc<dyn RevisionStore>,
    approvals: Arc<dyn ApprovalStore>,
    publication: Arc<dyn GatewayUseCases>,
    events: Arc<dyn EventRecorder>,
}

/// How an apply ended.
enum Applied {
    Activated(ActivatedDeployment, u64),
    Rejected(ValidationReport, Option<u64>),
    Checked(ValidationReport),
    /// Policies ask for approvals first; nothing was published.
    AwaitingApproval(Box<ApprovalRequest>),
}

/// Reads of the configuration as a whole, which a caller limited to some
/// sites may not make.
const WHOLE_READS: &[&str] = &[
    "config.source",
    "config.explain",
    "config.ir",
    "config.plan",
    "revisions.list",
    "revisions.get",
    "revisions.diff",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FilesBody {
    files: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NginxBody {
    files: BTreeMap<String, String>,
    entry: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SyntaxBody {
    files: BTreeMap<String, String>,
    #[serde(default)]
    file: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExplainBody {
    files: BTreeMap<String, String>,
    file: String,
    line: usize,
    #[serde(default = "first_column")]
    column: usize,
}

fn first_column() -> usize {
    1
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RevisionPage {
    before: Option<u64>,
    limit: Option<u32>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DiffParameters {
    /// `previous` (the default), `active`, `draft` or a revision number.
    against: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteBody {
    note: Option<String>,
}

fn json_output(value: &impl Serialize, etag: String) -> operations::Output {
    operations::Output {
        content: serde_json::to_vec(value).expect("API values serialize"),
        etag,
    }
}

fn decode<T: for<'de> Deserialize<'de>>(content: &[u8]) -> Result<T> {
    serde_json::from_slice(if content.is_empty() { b"{}" } else { content })
        .map_err(|error| PanelError::invalid_argument(format!("invalid request body: {error}")))
}

/// The entity tag of the draft as a whole.
fn draft_etag(version: u64) -> String {
    format!("\"draft-{version}\"")
}

fn revision_id(resource: &str) -> Result<u64> {
    resource
        .strip_prefix("revisions/")
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| PanelError::not_found(format!("no resource {resource:?}")))
}

fn warnings(diagnostics: &[Diagnostic]) -> Vec<Diagnostic> {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Warning)
        .cloned()
        .collect()
}

fn report_of(error: &PanelError) -> ValidationReport {
    let diagnostics = if error.diagnostics.is_empty() {
        vec![Diagnostic::error(error.code.clone(), error.message.clone())]
    } else {
        error.diagnostics.clone()
    };
    ValidationReport::from_diagnostics(diagnostics)
}

impl ConfigurationService {
    pub fn new(
        drafts: Arc<dyn DraftStore>,
        revisions: Arc<dyn RevisionStore>,
        approvals: Arc<dyn ApprovalStore>,
        publication: Arc<dyn GatewayUseCases>,
        events: Arc<dyn EventRecorder>,
    ) -> Self {
        Self {
            drafts,
            revisions,
            approvals,
            publication,
            events,
        }
    }

    /// Records how an apply ended unless it activated, which the draft
    /// records with the activation.
    async fn record_apply(
        &self,
        context: &CommandContext,
        request: &ApplyRequest,
        result: &Result<(DraftState, Applied)>,
    ) {
        let codes = |report: &ValidationReport| {
            report
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code.as_str().to_owned())
                .collect::<Vec<_>>()
        };
        let (scope, actor) = (context.scope(), context.actor());
        match result {
            Ok((_, Applied::Activated(..) | Applied::AwaitingApproval(_))) => {}
            Ok((draft, Applied::Checked(_))) => {
                store::record(
                    &*self.events,
                    DRAFT,
                    &scope,
                    actor,
                    &event::ApplyChecked {
                        version: draft.version,
                        valid: true,
                        revision: None,
                        codes: Vec::new(),
                    },
                )
                .await;
            }
            Ok((draft, Applied::Rejected(report, revision))) if request.dry_run => {
                store::record(
                    &*self.events,
                    DRAFT,
                    &scope,
                    actor,
                    &event::ApplyChecked {
                        version: draft.version,
                        valid: false,
                        revision: *revision,
                        codes: codes(report),
                    },
                )
                .await;
            }
            Ok((draft, Applied::Rejected(report, revision))) => {
                store::record(
                    &*self.events,
                    DRAFT,
                    &scope,
                    actor,
                    &event::ApplyRejected {
                        version: draft.version,
                        revision: *revision,
                        codes: codes(report),
                    },
                )
                .await;
            }
            Err(error) => {
                store::record(
                    &*self.events,
                    DRAFT,
                    &scope,
                    actor,
                    &event::ApplyFailed {
                        expected_version: request.expected_version,
                        dry_run: request.dry_run,
                        code: error.code.as_str().to_owned(),
                        message: error.message.clone(),
                    },
                )
                .await;
            }
        }
    }

    /// The model and files of the active revision; empty before the first.
    async fn active(&self) -> Result<(ConfigModel, Sources)> {
        match self.revisions.active().await? {
            Some((_, sources)) => {
                let lowered = language::read(&sources, None, Utc::now());
                Ok((lowered.model, sources))
            }
            None => Ok((ConfigModel::default(), Sources::default())),
        }
    }

    async fn sources_of(
        &self,
        which: &str,
        revision: &Revision,
        draft: &DraftState,
    ) -> Result<(ConfigModel, Sources)> {
        match which {
            "draft" => Ok((draft.model.clone(), draft.sources.clone())),
            "active" => self.active().await,
            "previous" => {
                let earlier = self.revisions.list(Some(revision.id), 1).await?;
                match earlier.first() {
                    Some(earlier) => {
                        let (_, sources) = self.revisions.get(earlier.id).await?;
                        Ok((language::read(&sources, None, Utc::now()).model, sources))
                    }
                    None => Ok((ConfigModel::default(), Sources::default())),
                }
            }
            other => {
                let id: u64 = other.parse().map_err(|_| {
                    PanelError::invalid_argument(format!(
                        "{other:?} is not previous, active, draft or a revision"
                    ))
                })?;
                let (_, sources) = self.revisions.get(id).await?;
                Ok((language::read(&sources, None, Utc::now()).model, sources))
            }
        }
    }

    /// Reads served by the language and revision history rather than the
    /// model; `None` for every other operation.
    async fn read_language(
        &self,
        draft: &DraftState,
        operation: &str,
        resource: &str,
        parameters: &[u8],
    ) -> Result<Option<operations::Output>> {
        let output = match (operation, resource) {
            ("config.source", "config/source") => {
                let lowered = language::read(&draft.sources, Some(&draft.model), Utc::now());
                json_output(
                    &json!({
                        "language_version": LANGUAGE_VERSION,
                        "version": draft.version,
                        "etag": draft_etag(draft.version),
                        "files": draft.sources,
                        "diagnostics": warnings(&lowered.diagnostics),
                    }),
                    draft_etag(draft.version),
                )
            }
            ("config.check", "config") => {
                let body: FilesBody = decode(parameters)?;
                let lowered = language::read(
                    &language::sources(body.files)?,
                    Some(&draft.model),
                    Utc::now(),
                );
                json_output(
                    &json!({ "valid": lowered.is_valid(), "diagnostics": lowered.diagnostics }),
                    String::new(),
                )
            }
            ("config.format", "config") => {
                let body: FilesBody = decode(parameters)?;
                let (formatted, diagnostics) = format_files(&language::sources(body.files)?);
                json_output(
                    &json!({ "files": formatted, "diagnostics": diagnostics }),
                    String::new(),
                )
            }
            ("config.ast", "config") => {
                let body: SyntaxBody = decode(parameters)?;
                let file = body.file.unwrap_or_else(|| ENTRY.to_owned());
                let tree = syntax_tree(&language::sources(body.files)?, &file)
                    .ok_or_else(|| PanelError::not_found(format!("there is no file {file:?}")))?;
                json_output(&tree, String::new())
            }
            ("config.explain", "config") => {
                let body: ExplainBody = decode(parameters)?;
                let sources = language::sources(body.files)?;
                let lowered = language::read(&sources, Some(&draft.model), Utc::now());
                let explanation =
                    explain(&sources, &lowered, &body.file, body.line, body.column).ok_or_else(
                        || {
                            PanelError::not_found(format!(
                                "no server, route, listener, upstream or TLS profile is written at {}:{}.{}",
                                body.file, body.line, body.column
                            ))
                        },
                    )?;
                json_output(&explanation, String::new())
            }
            ("config.import.nginx", "config") => {
                let body: NginxBody = decode(parameters)?;
                let imported =
                    import_nginx(&body.files, &body.entry).map_err(PanelError::invalid_argument)?;
                let lowered = language::read(&imported.sources, Some(&draft.model), Utc::now());
                json_output(
                    &json!({
                        "files": imported.sources,
                        "report": imported.report,
                        "valid": lowered.is_valid(),
                        "diagnostics": lowered.diagnostics,
                    }),
                    String::new(),
                )
            }
            ("config.ir", "config") => {
                let lowered = language::read(&draft.sources, Some(&draft.model), Utc::now());
                if !lowered.is_valid() {
                    return Err(PanelError::validation_failed("the draft has errors")
                        .with_diagnostics(lowered.diagnostics));
                }
                let snapshot = compile(&lowered.model, RevisionId::new(draft.version)).map_err(
                    |diagnostics| {
                        PanelError::validation_failed("the draft does not compile")
                            .with_diagnostics(diagnostics)
                    },
                )?;
                json_output(&snapshot, String::new())
            }
            ("config.schema", "config") => json_output(
                &json!({ "language_version": LANGUAGE_VERSION, "directives": DIRECTIVES }),
                String::new(),
            ),
            ("config.plan", "config") => {
                let (model, sources) = self.active().await?;
                json_output(
                    &changes((&model, &sources), (&draft.model, &draft.sources)),
                    String::new(),
                )
            }
            ("revisions.list", "revisions") => {
                let page: RevisionPage = decode(parameters)?;
                let limit = page
                    .limit
                    .unwrap_or(DEFAULT_REVISION_PAGE)
                    .clamp(1, MAX_REVISION_PAGE);
                let items = self.revisions.list(page.before, limit).await?;
                let next_before = (items.len() == limit as usize)
                    .then(|| items.last().map(|last| last.id))
                    .flatten();
                json_output(&RevisionList { items, next_before }, String::new())
            }
            ("revisions.get", resource) if resource.starts_with("revisions/") => {
                let (revision, sources) = self.revisions.get(revision_id(resource)?).await?;
                json_output(
                    &RevisionDetail {
                        revision,
                        files: sources.into_files(),
                    },
                    String::new(),
                )
            }
            ("revisions.diff", resource) if resource.starts_with("revisions/") => {
                let parameters: DiffParameters = decode(parameters)?;
                let (revision, sources) = self.revisions.get(revision_id(resource)?).await?;
                let against = parameters.against.as_deref().unwrap_or("previous");
                let (old_model, old_sources) = self.sources_of(against, &revision, draft).await?;
                let model = language::read(&sources, None, Utc::now()).model;
                json_output(
                    &changes((&old_model, &old_sources), (&model, &sources)),
                    String::new(),
                )
            }
            _ => return Ok(None),
        };
        Ok(Some(output))
    }

    async fn apply_draft(
        &self,
        context: &CommandContext,
        request: &ApplyRequest,
    ) -> Result<(DraftState, Applied)> {
        let draft = self.drafts.load().await?;
        let expected_version = request.expected_version;
        if expected_version != 0 && expected_version != draft.version {
            return Err(PanelError::conflict(format!(
                "the draft is at version {}, not {expected_version}",
                draft.version
            )));
        }
        if draft.version == 0 {
            return Err(PanelError::precondition_failed(
                "the draft has no changes to apply",
            ));
        }
        if context.site_scope().is_some() {
            let (active, _) = self.active().await?;
            scope::check_changes(&active, &draft.model, context.site_scope(), scope::APPLY)?;
        }
        let status = self.publication.status().await?;
        self.revisions
            .settle(status.active_hash().map(ContentHash::as_str))
            .await?;
        let note = request
            .note
            .as_deref()
            .map(str::trim)
            .filter(|note| !note.is_empty());
        let content_hash = language::content_hash(&draft.sources);
        let record = NewRevision {
            draft_version: draft.version,
            sources: &draft.sources,
            content_hash: content_hash.as_str(),
            author: context.actor(),
            note,
            snapshot_hash: None,
        };
        let dry_run = request.dry_run;
        let revisions = &self.revisions;
        let rejected = |report: ValidationReport| async {
            if dry_run {
                return Ok(Applied::Rejected(report, None));
            }
            let id = revisions.reject(&record, &report.diagnostics).await?;
            Ok::<_, PanelError>(Applied::Rejected(report, Some(id)))
        };

        let lowered = language::read(&draft.sources, Some(&draft.model), Utc::now());
        if !lowered.is_valid() {
            let outcome = rejected(ValidationReport::from_diagnostics(lowered.diagnostics)).await?;
            return Ok((draft, outcome));
        }
        let snapshot: RuntimeSnapshot =
            match compile(&lowered.model, RevisionId::new(draft.version)) {
                Ok(snapshot) => snapshot,
                Err(diagnostics) => {
                    let outcome = rejected(ValidationReport::from_diagnostics(diagnostics)).await?;
                    return Ok((draft, outcome));
                }
            };
        let declared = snapshot
            .required_capabilities()
            .iter()
            .map(|capability| {
                EngineCapability::new(capability.name.clone(), capability.version.clone())
            })
            .collect();
        let report = validate_engine_ir(&snapshot, &declared)?;
        if !report.valid {
            let outcome = rejected(report).await?;
            return Ok((draft, outcome));
        }
        let document = ConfigDocument::new(
            IR_SCHEMA_VERSION,
            "application/json",
            serde_json::to_vec(&snapshot).map_err(|error| {
                PanelError::internal(format!("snapshot cannot be encoded: {error}"))
            })?,
        )?;
        let mut checked = warnings(&lowered.diagnostics);
        checked.extend(report.diagnostics.iter().cloned());

        if request.dry_run {
            let outcome = match self.publication.prepare(context.clone(), document).await {
                Ok(prepared) => {
                    self.publication
                        .abort(context.clone(), prepared.prepare_token().to_owned())
                        .await?;
                    Applied::Checked(ValidationReport::from_diagnostics(checked))
                }
                Err(error) if !error.retryable => Applied::Rejected(report_of(&error), None),
                Err(error) => return Err(error),
            };
            return Ok((draft, outcome));
        }

        let approved = match self
            .approval_gate(context, request, &draft, content_hash.as_str(), note)
            .await?
        {
            Ok(approved) => approved,
            Err(waiting) => return Ok((draft, Applied::AwaitingApproval(waiting))),
        };
        let snapshot_hash = snapshot.content_hash.clone();
        let id = self
            .revisions
            .begin(&NewRevision {
                snapshot_hash: Some(snapshot_hash.as_str()),
                ..record
            })
            .await?;
        let prepared = match self.publication.prepare(context.clone(), document).await {
            Ok(prepared) => prepared,
            Err(error) => {
                self.revisions
                    .fail(id, &report_of(&error).diagnostics)
                    .await?;
                return Err(error);
            }
        };
        let activated = match self
            .publication
            .activate(
                context.clone(),
                prepared.prepare_token().to_owned(),
                status.active_hash().cloned(),
            )
            .await
        {
            Ok(activated) => activated,
            Err(error) => {
                // An outcome the gateway may still have committed is settled
                // by the next apply, which compares the active hash.
                if error.code.as_str() != panel_errors::ErrorCode::COMMIT_OUTCOME_UNKNOWN {
                    self.revisions
                        .fail(id, &report_of(&error).diagnostics)
                        .await?;
                }
                return Err(error);
            }
        };
        self.revisions
            .activate(
                id,
                activated.content_hash().as_str(),
                activated.revision_id().get(),
            )
            .await?;
        if let Some(request) = approved {
            self.approvals
                .applied(request, id, context.actor(), &context.scope())
                .await?;
        }
        let draft = self
            .drafts
            .mark_applied(draft.version, id, note, &context.scope(), context.actor())
            .await?;
        Ok((draft, Applied::Activated(activated, id)))
    }
}

/// The draft's version and application state, as the port reports it.
fn draft_info(draft: &DraftState) -> DraftInfo {
    DraftInfo {
        version: draft.version,
        updated_at: Some(draft.updated_at.into()),
        applied_version: draft.applied_version,
        applied_at: draft.applied_at.map(Into::into),
    }
}

fn answer(draft: &DraftState, content: Vec<u8>, etag: String) -> ConfigurationOutput {
    ConfigurationOutput {
        content,
        etag: (!etag.is_empty()).then_some(etag),
        draft: draft_info(draft),
    }
}

/// Binds an idempotency key to the exact change it was first used for.
fn request_hash(change: &ConfigurationChange) -> ContentHash {
    let mut bytes = Vec::with_capacity(change.content.len() + 128);
    for part in [
        change.operation.as_bytes(),
        change.resource.as_bytes(),
        change.if_match.as_deref().unwrap_or_default().as_bytes(),
        &change.content,
    ] {
        bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
        bytes.extend_from_slice(part);
    }
    ContentHash::from_bytes(&bytes)
}

impl ConfigurationService {
    async fn change_draft(
        &self,
        context: &CommandContext,
        request: &ConfigurationChange,
    ) -> Result<(DraftState, ChangeOutput)> {
        if let Some(output) = self.change_approvals(context, request).await? {
            let draft = self.drafts.load().await?;
            return Ok((draft, output));
        }
        if matches!(
            request.operation.as_str(),
            "revisions.note" | "revisions.restore" | "config.source.replace"
        ) {
            scope::require_everywhere(
                context.site_scope(),
                scope::WRITE,
                "changing the configuration files or revisions",
            )?;
        }
        if request.operation == "revisions.note" {
            let body: NoteBody = decode(&request.content)?;
            let note = body
                .note
                .as_deref()
                .map(str::trim)
                .filter(|note| !note.is_empty());
            let id = revision_id(&request.resource)?;
            let revision = self.revisions.set_note(id, note).await?;
            store::record(
                &*self.events,
                ("revision", &id.to_string()),
                &context.scope(),
                context.actor(),
                &event::RevisionNoted {
                    revision: id,
                    note: note.map(str::to_owned),
                },
            )
            .await;
            let draft = self.drafts.load().await?;
            return Ok((
                draft,
                ChangeOutput {
                    content: serde_json::to_vec(&revision).expect("API values serialize"),
                    etag: String::new(),
                },
            ));
        }
        let restored = if request.operation == "revisions.restore" {
            Some(self.revisions.get(revision_id(&request.resource)?).await?.1)
        } else {
            None
        };
        let if_match = request.if_match.as_deref().unwrap_or_default();
        let scope = context.scope();
        self.drafts
            .change(
                ChangeRequest {
                    idempotency_key: context.idempotency_key(),
                    operation: &request.operation,
                    resource: &request.resource,
                    request_hash: request_hash(request),
                    scope: &scope,
                    actor: context.actor(),
                },
                Box::new(|draft| {
                    let next_etag = draft_etag(draft.version + 1);
                    let text = match (request.operation.as_str(), request.resource.as_str()) {
                        ("config.source.replace", "config/source") => {
                            if !if_match.is_empty()
                                && if_match != "*"
                                && if_match != draft_etag(draft.version)
                            {
                                return Err(PanelError::precondition_failed(
                                    "the draft changed since it was read; reload it and try again",
                                ));
                            }
                            let body: FilesBody = decode(&request.content)?;
                            Some(language::sources(body.files)?)
                        }
                        ("revisions.restore", _) => restored,
                        _ => None,
                    };
                    if let Some(sources) = text {
                        let (model, written, warnings) = language::replace(&sources, &draft.model)?;
                        let content = serde_json::to_vec(&json!({
                            "language_version": LANGUAGE_VERSION,
                            "version": draft.version + 1,
                            "etag": next_etag,
                            "files": written,
                            "diagnostics": warnings,
                        }))
                        .expect("API values serialize");
                        return Ok(DraftChange {
                            model,
                            sources: written,
                            output: ChangeOutput {
                                content,
                                etag: next_etag,
                            },
                        });
                    }
                    let (model, output) = operations::change(
                        &draft.model,
                        &request.operation,
                        &request.resource,
                        if_match,
                        &request.content,
                        Utc::now(),
                    )?;
                    scope::check_changes(&draft.model, &model, context.site_scope(), scope::WRITE)?;
                    Ok(DraftChange {
                        sources: language::follow(&draft.sources, &draft.model, &model),
                        model,
                        output: ChangeOutput {
                            content: output.content,
                            etag: output.etag,
                        },
                    })
                }),
            )
            .await
    }
}

#[async_trait]
impl ConfigurationPort for ConfigurationService {
    async fn read(
        &self,
        scope: RequestScope,
        read: ConfigurationRead,
    ) -> Result<ConfigurationOutput> {
        let site_scope = scope.site_scope();
        if WHOLE_READS.contains(&read.operation.as_str()) || read.operation.starts_with("approval")
        {
            scope::require_everywhere(
                site_scope,
                scope::READ,
                "this view of the whole configuration",
            )?;
        }
        let draft = self.drafts.load().await?;
        let output = match self
            .read_approvals(&draft, &read.operation, &read.resource, &read.parameters)
            .await?
        {
            Some(output) => output,
            None => match self
                .read_language(&draft, &read.operation, &read.resource, &read.parameters)
                .await?
            {
                Some(output) => output,
                None => operations::read(
                    &scope::readable(&draft.model, site_scope),
                    &read.operation,
                    &read.resource,
                    &read.parameters,
                )?,
            },
        };
        Ok(answer(&draft, output.content, output.etag))
    }

    async fn change(
        &self,
        context: CommandContext,
        change: ConfigurationChange,
    ) -> Result<ConfigurationOutput> {
        let result = self.change_draft(&context, &change).await;
        if let Err(error) = &result {
            store::record(
                &*self.events,
                DRAFT,
                &context.scope(),
                context.actor(),
                &event::ChangeRefused {
                    operation: change.operation.clone(),
                    resource: change.resource.clone(),
                    code: error.code.as_str().to_owned(),
                    message: error.message.clone(),
                },
            )
            .await;
        }
        let (draft, output) = result?;
        Ok(answer(&draft, output.content, output.etag))
    }

    async fn apply(&self, context: CommandContext, request: ApplyRequest) -> Result<ApplyOutcome> {
        let result = self.apply_draft(&context, &request).await;
        self.record_apply(&context, &request, &result).await;
        let (draft, applied) = result?;
        let draft = draft_info(&draft);
        Ok(match applied {
            Applied::Activated(deployment, revision) => {
                ApplyOutcome::applied(draft, deployment, revision)
            }
            Applied::Rejected(report, revision) => ApplyOutcome::rejected(draft, report, revision),
            Applied::Checked(report) => ApplyOutcome::checked(draft, report),
            Applied::AwaitingApproval(waiting) => ApplyOutcome::awaiting_approval(
                draft,
                serde_json::to_vec(&waiting).expect("API values serialize"),
            ),
        })
    }
}
