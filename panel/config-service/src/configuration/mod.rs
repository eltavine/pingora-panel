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
    ActivatedDeployment, CommandContext, ConfigDocument, ContentHash, GatewayUseCases, RequestScope,
};
use panel_config_api::{
    ApplyOutcome, ApplyRequest, ConfigurationChange, ConfigurationCommand, ConfigurationOutput,
    ConfigurationPort, ConfigurationQuery, DiffBase, DraftInfo, LanguageChange, LanguageQuery,
    LuaCommand, LuaRunOutcome, LuaTest, ModelChange, RevisionChange, RevisionQuery,
};
use panel_config_dsl::{
    explain, format_files, import_nginx, lua_library, plan::changes, schema::DIRECTIVES,
    syntax_tree, Sources, ENTRY, LANGUAGE_VERSION,
};
use panel_config_model::{
    compile, ApprovalRequest, ConfigModel, Revision, RevisionDetail, RevisionList,
};
use panel_domain::RevisionId;
use panel_engine::{validate_engine_ir, EngineCapability};
use panel_errors::{Diagnostic, DiagnosticSeverity, PanelError, Result, ValidationReport};
use panel_event_contracts::config::v1 as event;
use panel_ir::{RuntimeSnapshot, IR_SCHEMA_VERSION};
use serde::Serialize;
use serde_json::json;
use std::sync::Arc;

mod approval_operations;
#[cfg(test)]
mod tests;

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

/// Whether a read sees the configuration as a whole, which a caller limited
/// to some sites may not make.
fn whole(query: &ConfigurationQuery) -> bool {
    match query {
        ConfigurationQuery::Model(_) => false,
        ConfigurationQuery::Language(query) => matches!(
            query,
            LanguageQuery::Source
                | LanguageQuery::Explain { .. }
                | LanguageQuery::Ir
                | LanguageQuery::Plan
                | LanguageQuery::Lua { .. }
        ),
        ConfigurationQuery::Revision(_) | ConfigurationQuery::Approval(_) => true,
    }
}

/// What a change does to the draft.
enum Edit {
    /// Replaces its files, if the draft still has the entity tag the caller
    /// read, when the caller sent one.
    Files {
        sources: Sources,
        if_match: Option<String>,
    },
    Model(Box<ModelChange>),
}

fn json_output(value: &impl Serialize, etag: String) -> operations::Output {
    operations::Output {
        content: serde_json::to_vec(value).expect("API values serialize"),
        etag,
    }
}

/// The entity tag of the draft as a whole.
fn draft_etag(version: u64) -> String {
    format!("\"draft-{version}\"")
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
        against: DiffBase,
        revision: &Revision,
        draft: &DraftState,
    ) -> Result<(ConfigModel, Sources)> {
        let id = match against {
            DiffBase::Draft => return Ok((draft.model.clone(), draft.sources.clone())),
            DiffBase::Active => return self.active().await,
            DiffBase::Previous => match self.revisions.list(Some(revision.id), 1).await?.first() {
                Some(earlier) => earlier.id,
                None => return Ok((ConfigModel::default(), Sources::default())),
            },
            DiffBase::Revision(id) => id,
        };
        let (_, sources) = self.revisions.get(id).await?;
        Ok((language::read(&sources, None, Utc::now()).model, sources))
    }

    /// Reads in the configuration language, of the draft's files or of the
    /// files sent with them.
    async fn read_language(
        &self,
        draft: &DraftState,
        query: LanguageQuery,
    ) -> Result<operations::Output> {
        Ok(match query {
            LanguageQuery::Source => {
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
            LanguageQuery::Check { files } => {
                let lowered =
                    language::read(&language::sources(files)?, Some(&draft.model), Utc::now());
                json_output(
                    &json!({ "valid": lowered.is_valid(), "diagnostics": lowered.diagnostics }),
                    String::new(),
                )
            }
            LanguageQuery::Format { files } => {
                let (formatted, diagnostics) = format_files(&language::sources(files)?);
                json_output(
                    &json!({ "files": formatted, "diagnostics": diagnostics }),
                    String::new(),
                )
            }
            LanguageQuery::Syntax { files, file } => {
                let file = file.unwrap_or_else(|| ENTRY.to_owned());
                let tree = syntax_tree(&language::sources(files)?, &file)
                    .ok_or_else(|| PanelError::not_found(format!("there is no file {file:?}")))?;
                json_output(&tree, String::new())
            }
            LanguageQuery::Explain {
                files,
                file,
                line,
                column,
            } => {
                let sources = language::sources(files)?;
                let lowered = language::read(&sources, Some(&draft.model), Utc::now());
                let explanation = explain(&sources, &lowered, &file, line, column).ok_or_else(|| {
                    PanelError::not_found(format!(
                        "no server, route, listener, upstream or TLS profile is written at {file}:{line}.{column}"
                    ))
                })?;
                json_output(&explanation, String::new())
            }
            LanguageQuery::ImportNginx { files, entry } => {
                let imported =
                    import_nginx(&files, &entry).map_err(PanelError::invalid_argument)?;
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
            LanguageQuery::Ir => {
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
            LanguageQuery::Schema => json_output(
                &json!({ "language_version": LANGUAGE_VERSION, "directives": DIRECTIVES }),
                String::new(),
            ),
            LanguageQuery::Plan => {
                let (model, sources) = self.active().await?;
                json_output(
                    &changes((&model, &sources), (&draft.model, &draft.sources)),
                    String::new(),
                )
            }
            LanguageQuery::Lua { revision } => {
                let sources = match revision {
                    Some(id) => self.revisions.get(id).await?.1,
                    None => draft.sources.clone(),
                };
                let lowered = language::read(&sources, Some(&draft.model), Utc::now());
                let mut library =
                    serde_json::to_value(lua_library(&lowered)).expect("API values serialize");
                library["version"] = json!(draft.version);
                library["revision"] = json!(revision);
                json_output(&library, String::new())
            }
        })
    }

    /// Runs a Lua test on the draft and records that it ran.
    async fn test_lua(&self, context: &CommandContext, test: LuaTest) -> Result<ChangeOutput> {
        scope::require_everywhere(context.site_scope(), scope::LUA, "testing Lua scripts")?;
        let draft = self.drafts.load().await?;
        let script_phase = test
            .script
            .as_ref()
            .map(|script| script.phase.clone())
            .unwrap_or_default();
        let result = crate::lua_test::run(
            &draft.model,
            draft.version,
            test,
            context.request_id().as_str(),
        )
        .await?;
        let outcome = result
            .runs
            .last()
            .map_or("continue", |run| match run.outcome {
                LuaRunOutcome::Respond => "respond",
                LuaRunOutcome::Abort => "abort",
                LuaRunOutcome::Failed => "failed",
                _ => "continue",
            });
        store::record(
            &*self.events,
            ("lua", "test"),
            &context.scope(),
            context.actor(),
            &event::LuaTested {
                version: draft.version,
                script_phase,
                site: result.site_id.clone().unwrap_or_default(),
                route: result.route_id.clone().unwrap_or_default(),
                phases: result.runs.iter().map(|run| run.phase.clone()).collect(),
                outcome: outcome.to_owned(),
                duration_us: result.runs.iter().map(|run| run.duration_us).sum(),
            },
        )
        .await;
        let output = json_output(&result, String::new());
        Ok(ChangeOutput {
            content: output.content,
            etag: output.etag,
        })
    }

    /// Reads of the configurations applied or attempted.
    async fn read_revisions(
        &self,
        draft: &DraftState,
        query: RevisionQuery,
    ) -> Result<operations::Output> {
        Ok(match query {
            RevisionQuery::Revisions { before, limit } => {
                let limit = limit
                    .unwrap_or(DEFAULT_REVISION_PAGE)
                    .clamp(1, MAX_REVISION_PAGE);
                let items = self.revisions.list(before, limit).await?;
                let next_before = (items.len() == limit as usize)
                    .then(|| items.last().map(|last| last.id))
                    .flatten();
                json_output(&RevisionList { items, next_before }, String::new())
            }
            RevisionQuery::Revision { id } => {
                let (revision, sources) = self.revisions.get(id).await?;
                json_output(
                    &RevisionDetail {
                        revision,
                        files: sources.into_files(),
                    },
                    String::new(),
                )
            }
            RevisionQuery::Diff { id, against } => {
                let (revision, sources) = self.revisions.get(id).await?;
                let (old_model, old_sources) = self.sources_of(against, &revision, draft).await?;
                let model = language::read(&sources, None, Utc::now()).model;
                json_output(
                    &changes((&old_model, &old_sources), (&model, &sources)),
                    String::new(),
                )
            }
        })
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
            scope::check_lua(&active, &draft.model, context.site_scope())?;
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
    let command = serde_json::to_vec(&change.command).expect("configuration commands serialize");
    let mut bytes = Vec::with_capacity(command.len() + 64);
    for part in [
        change.if_match.as_deref().unwrap_or_default().as_bytes(),
        &command,
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
        change: ConfigurationChange,
    ) -> Result<(DraftState, ChangeOutput)> {
        let request_hash = request_hash(&change);
        let ConfigurationChange { command, if_match } = change;
        let (operation, resource) = (command.operation(), command.resource());
        let whole = || {
            scope::require_everywhere(
                context.site_scope(),
                scope::WRITE,
                "changing the configuration files or revisions",
            )
        };
        let edit = match command {
            ConfigurationCommand::Approval(change) => {
                let output = self.change_approvals(context, change).await?;
                return Ok((self.drafts.load().await?, output));
            }
            ConfigurationCommand::Lua(LuaCommand::Test { test }) => {
                let output = self.test_lua(context, test).await?;
                return Ok((self.drafts.load().await?, output));
            }
            ConfigurationCommand::Revision(RevisionChange::Note { id, note }) => {
                whole()?;
                let note = note
                    .as_deref()
                    .map(str::trim)
                    .filter(|note| !note.is_empty());
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
                let output = json_output(&revision, String::new());
                return Ok((
                    self.drafts.load().await?,
                    ChangeOutput {
                        content: output.content,
                        etag: output.etag,
                    },
                ));
            }
            ConfigurationCommand::Revision(RevisionChange::Restore { id }) => {
                whole()?;
                Edit::Files {
                    sources: self.revisions.get(id).await?.1,
                    if_match: None,
                }
            }
            ConfigurationCommand::Language(LanguageChange::ReplaceSource { files }) => {
                whole()?;
                Edit::Files {
                    sources: language::sources(files)?,
                    if_match: if_match.clone(),
                }
            }
            ConfigurationCommand::Model(change) => Edit::Model(change),
        };
        let if_match = if_match.unwrap_or_default();
        let scope = context.scope();
        self.drafts
            .change(
                ChangeRequest {
                    idempotency_key: context.idempotency_key(),
                    operation,
                    resource: &resource,
                    request_hash,
                    scope: &scope,
                    actor: context.actor(),
                },
                Box::new(move |draft| match edit {
                    Edit::Files { sources, if_match } => {
                        if if_match
                            .is_some_and(|tag| tag != "*" && tag != draft_etag(draft.version))
                        {
                            return Err(PanelError::precondition_failed(
                                "the draft changed since it was read; reload it and try again",
                            ));
                        }
                        let (model, written, warnings) = language::replace(&sources, &draft.model)?;
                        scope::check_lua(&draft.model, &model, context.site_scope())?;
                        let next_etag = draft_etag(draft.version + 1);
                        let content = serde_json::to_vec(&json!({
                            "language_version": LANGUAGE_VERSION,
                            "version": draft.version + 1,
                            "etag": next_etag,
                            "files": written,
                            "diagnostics": warnings,
                        }))
                        .expect("API values serialize");
                        Ok(DraftChange {
                            model,
                            sources: written,
                            output: ChangeOutput {
                                content,
                                etag: next_etag,
                            },
                        })
                    }
                    Edit::Model(change) => {
                        let (model, output) =
                            operations::change(&draft.model, *change, &if_match, Utc::now())?;
                        scope::check_changes(
                            &draft.model,
                            &model,
                            context.site_scope(),
                            scope::WRITE,
                        )?;
                        scope::check_lua(&draft.model, &model, context.site_scope())?;
                        Ok(DraftChange {
                            sources: language::follow(&draft.sources, &draft.model, &model),
                            model,
                            output: ChangeOutput {
                                content: output.content,
                                etag: output.etag,
                            },
                        })
                    }
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
        query: ConfigurationQuery,
    ) -> Result<ConfigurationOutput> {
        if whole(&query) {
            scope::require_everywhere(
                scope.site_scope(),
                scope::READ,
                "this view of the whole configuration",
            )?;
        }
        let draft = self.drafts.load().await?;
        let output = match query {
            ConfigurationQuery::Model(query) => {
                operations::read(&scope::readable(&draft.model, scope.site_scope()), &query)?
            }
            ConfigurationQuery::Language(query) => self.read_language(&draft, query).await?,
            ConfigurationQuery::Revision(query) => self.read_revisions(&draft, query).await?,
            ConfigurationQuery::Approval(query) => self.read_approvals(&draft, query).await?,
        };
        Ok(answer(&draft, output.content, output.etag))
    }

    async fn change(
        &self,
        context: CommandContext,
        change: ConfigurationChange,
    ) -> Result<ConfigurationOutput> {
        let (operation, resource) = (change.command.operation(), change.command.resource());
        let result = self.change_draft(&context, change).await;
        if let Err(error) = &result {
            store::record(
                &*self.events,
                DRAFT,
                &context.scope(),
                context.actor(),
                &event::ChangeRefused {
                    operation: operation.to_owned(),
                    resource,
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
            Applied::AwaitingApproval(waiting) => ApplyOutcome::awaiting_approval(draft, *waiting),
        })
    }
}
