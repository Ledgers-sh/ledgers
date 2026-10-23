//! The group commands' server side (issue #128): company groups, intercompany
//! agreements, the period drafts, posting and settlement, and the two group
//! reports.
//!
//! A group command touches more than one company, so before it runs the
//! server checks the person's role in every company it touches
//! (docs/permissions.md). A company the person may not see is never named in
//! a response: lists and reports show only the companies that pass the check,
//! and a single agreement that the request's company is not a party to answers
//! not found.
//!
//! Posting and settling run each side's entry through that company's own
//! executor, here as a [`ServerDispatch`] for the other company, so the
//! policy, the audit trail and the ledger's checks are the ones every entry
//! gets.

use super::{Deps, EntriesReverse, Presented, ServerDispatch, fill_server_fields, rfc3339_utc};
use async_trait::async_trait;
use cratefield_core::{DbError, Problem};
use ledgers_commands::{CommandError, Role};
use ledgers_group::commands::{
    AgreementIdInput, AgreementsCreate, AgreementsCreateInput, AgreementsGet, AgreementsList,
    AgreementsRunPeriod, AgreementsTerminate, AgreementsUpdate, AgreementsUpdateInput,
    GroupViewInput, GroupsCreate, GroupsCreateInput, GroupsList, GroupsUpdate, GroupsUpdateInput,
    PeriodPair, PeriodsInput, PeriodsOutput, ReconciliationInput, RunPeriodInput, check_periods,
};
use ledgers_group::orchestrate::{self, SideExecutor};
use ledgers_group::reports::{GroupViewReport, ReconciliationReport};
use ledgers_group::store as gstore;
use ledgers_ledger::surface::{PostEntry, PostEntryInput};
use ledgers_policy::roles;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// The problem a refused role check answers with.
fn denied(request_id: &str) -> Problem {
    CommandError::PolicyDenied.to_problem().instance(request_id)
}

fn not_found(request_id: &str) -> Problem {
    Problem::not_found().instance(request_id)
}

fn database(err: DbError, request_id: &str) -> Problem {
    CommandError::Database(err)
        .to_problem()
        .instance(request_id)
}

fn invalid(message: impl Into<String>, request_id: &str) -> Problem {
    CommandError::InvalidInput(message.into())
        .to_problem()
        .instance(request_id)
}

fn parse<T: DeserializeOwned>(input: &Value, request_id: &str) -> Result<T, Problem> {
    serde_json::from_value(input.clone()).map_err(|err| invalid(err.to_string(), request_id))
}

/// A failure as the text a side's executor reports it with.
fn problem_text(problem: &Problem) -> String {
    match &problem.detail {
        Some(detail) => format!("{}: {detail}", problem.title),
        None => problem.title.to_owned(),
    }
}

/// Posts and reverses entries through the other companies' executors.
pub(super) struct DispatchSides<'a> {
    pub(super) dispatch: &'a ServerDispatch,
    pub(super) deps: &'a Deps,
    pub(super) presented: Option<Presented>,
    pub(super) request_id: &'a str,
}

impl DispatchSides<'_> {
    /// A request id unique to one side call, derived from the HTTP request.
    fn side_request(&self) -> String {
        format!("{}-{}", self.request_id, self.deps.id_gen.ulid())
    }
}

#[async_trait]
impl SideExecutor for DispatchSides<'_> {
    async fn post(&self, company_id: &str, input: &PostEntryInput) -> Result<String, String> {
        let value = serde_json::to_value(input).map_err(|err| err.to_string())?;
        // Each side post is its own request to its company's ledger: the audit
        // chain keys a request, so one request id cannot serve two posts.
        let request = self.side_request();
        let out = self
            .dispatch
            .sibling(company_id)
            .execute(
                self.deps,
                &PostEntry,
                self.presented.clone(),
                &request,
                value,
            )
            .await
            .map_err(|problem| problem_text(&problem))?;
        out.pointer("/entry/entry_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "the posted entry has no id".to_owned())
    }

    async fn reverse(
        &self,
        company_id: &str,
        external_id: &str,
        posting_date: &str,
    ) -> Result<(), String> {
        let value = json!({ "external_id": external_id, "posting_date": posting_date });
        let request = self.side_request();
        self.dispatch
            .sibling(company_id)
            .execute(
                self.deps,
                &EntriesReverse,
                self.presented.clone(),
                &request,
                value,
            )
            .await
            .map(|_| ())
            .map_err(|problem| problem_text(&problem))
    }
}

impl ServerDispatch {
    /// The same configuration for another company: the other side of a
    /// group command runs through its own executor.
    pub(super) fn sibling(&self, company_id: &str) -> ServerDispatch {
        ServerDispatch {
            company_id: company_id.to_owned(),
            home_company: self.home_company.clone(),
            app_origins: self.app_origins.clone(),
            app_url: self.app_url.clone(),
            document_key: self.document_key.clone(),
            model: self.model.clone(),
            vies_enabled: self.vies_enabled,
        }
    }

    /// The companies the person holds a role in, among `candidates`, that the
    /// matrix lets run `command`. Each candidate is checked once.
    async fn visible(
        &self,
        deps: &Deps,
        person: &str,
        candidates: &BTreeSet<String>,
        command: &str,
        request_id: &str,
    ) -> Result<Vec<String>, Problem> {
        let mut out = Vec::new();
        for company in candidates {
            let role = crate::members::role_of(deps.db.as_ref(), company, person)
                .await
                .map_err(|err| database(err, request_id))?;
            if let Some(role) = role
                && roles::check(role, command).is_ok()
            {
                out.push(company.clone());
            }
        }
        Ok(out)
    }

    /// Refuses unless the person holds, in every company, a role the matrix
    /// lets run `command`.
    async fn require_roles(
        &self,
        deps: &Deps,
        person: &str,
        companies: &[&str],
        command: &str,
        request_id: &str,
    ) -> Result<(), Problem> {
        for company in companies {
            let role = crate::members::role_of(deps.db.as_ref(), company, person)
                .await
                .map_err(|err| database(err, request_id))?;
            match role {
                Some(role) if roles::check(role, command).is_ok() => {}
                _ => return Err(denied(request_id)),
            }
        }
        Ok(())
    }

    /// Refuses unless the person is an owner of every company.
    async fn require_owners(
        &self,
        deps: &Deps,
        person: &str,
        companies: &BTreeSet<String>,
        request_id: &str,
    ) -> Result<(), Problem> {
        for company in companies {
            let role = crate::members::role_of(deps.db.as_ref(), company, person)
                .await
                .map_err(|err| database(err, request_id))?;
            if role != Some(Role::Owner) {
                return Err(denied(request_id));
            }
        }
        Ok(())
    }

    /// The two sides of an agreement, when the request's company is one of
    /// them. Anything else is not found, so the agreement's existence is not
    /// shown to other companies.
    async fn agreement_in_company(
        &self,
        deps: &Deps,
        agreement_id: &str,
        request_id: &str,
    ) -> Result<(String, String), Problem> {
        let sides = gstore::agreement_sides(deps.db.as_ref(), agreement_id)
            .await
            .map_err(|err| database(err, request_id))?;
        let licensor = sides
            .iter()
            .find(|row| row.view.side == ledgers_group::model::Side::Licensor);
        let licensee = sides
            .iter()
            .find(|row| row.view.side == ledgers_group::model::Side::Licensee);
        match (licensor, licensee) {
            (Some(licensor), Some(licensee))
                if licensor.company_id == self.company_id
                    || licensee.company_id == self.company_id =>
            {
                Ok((licensor.company_id.clone(), licensee.company_id.clone()))
            }
            _ => Err(not_found(request_id)),
        }
    }

    /// Routes a group command to its handler.
    #[allow(clippy::too_many_lines)]
    pub(super) async fn group_command(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        command: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        match command {
            "groups.create" => self.groups_create(deps, presented, request_id, input).await,
            "groups.list" => self.groups_list(deps, presented, request_id).await,
            "groups.update" => self.groups_update(deps, presented, request_id, input).await,
            "agreements.create" => {
                self.agreements_create(deps, presented, request_id, input)
                    .await
            }
            "agreements.list" => self.agreements_list(deps, presented, request_id).await,
            "agreements.get" => {
                self.agreements_get(deps, presented, request_id, input)
                    .await
            }
            "agreements.update" => {
                self.agreements_update(deps, presented, request_id, input)
                    .await
            }
            "agreements.terminate" => {
                self.agreements_terminate(deps, presented, request_id, input)
                    .await
            }
            "agreements.run_period" => {
                self.agreements_run_period(deps, presented, request_id, input)
                    .await
            }
            "agreements.post_period" => {
                self.agreements_period_action(deps, presented, request_id, input, true)
                    .await
            }
            "agreements.settle_period" => {
                self.agreements_period_action(deps, presented, request_id, input, false)
                    .await
            }
            "reports.intercompany_reconciliation" => {
                self.reconciliation(deps, presented, request_id, input)
                    .await
            }
            "reports.group_view" => self.group_view(deps, presented, request_id, input).await,
            other => Err(invalid(
                format!("{other} is not a group command"),
                request_id,
            )),
        }
    }

    /// `groups.create`: the person must own every member company.
    async fn groups_create(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        mut input: Value,
    ) -> Result<Value, Problem> {
        let person = self
            .person_id(deps, presented.clone(), request_id, None)
            .await?;
        let parsed: GroupsCreateInput = parse(&input, request_id)?;
        let members: BTreeSet<String> = parsed
            .members
            .iter()
            .map(|m| m.company_id.clone())
            .collect();
        self.require_owners(deps, &person, &members, request_id)
            .await?;
        fill_server_fields(&mut input, deps, "id", &person);
        self.execute(deps, &GroupsCreate, presented, request_id, input)
            .await
    }

    /// `groups.list`: the groups the company belongs to, with the members the
    /// person may see.
    async fn groups_list(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
    ) -> Result<Value, Problem> {
        let person = self
            .person_id(deps, presented.clone(), request_id, None)
            .await?;
        let groups = gstore::groups_of_company(deps.db.as_ref(), &self.company_id)
            .await
            .map_err(|err| database(err, request_id))?;
        let ids: Vec<String> = groups.iter().map(|g| g.id.clone()).collect();
        let members = gstore::members_of_groups(deps.db.as_ref(), &ids)
            .await
            .map_err(|err| database(err, request_id))?;
        let candidates: BTreeSet<String> = members.into_iter().map(|m| m.company_id).collect();
        let accessible = self
            .visible(deps, &person, &candidates, "groups.list", request_id)
            .await?;
        self.execute(
            deps,
            &GroupsList { accessible },
            presented,
            request_id,
            json!({}),
        )
        .await
    }

    /// `groups.update`: the person must own the members before and after.
    async fn groups_update(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let person = self
            .person_id(deps, presented.clone(), request_id, None)
            .await?;
        let parsed: GroupsUpdateInput = parse(&input, request_id)?;
        if let Some(doc) = parsed.logo_document_id.as_deref() {
            self.check_logo_document(deps, doc, request_id).await?;
        }
        let mut companies: BTreeSet<String> = gstore::members_of(deps.db.as_ref(), &parsed.id)
            .await
            .map_err(|err| database(err, request_id))?
            .into_iter()
            .map(|m| m.company_id)
            .collect();
        if let Some(members) = &parsed.members {
            companies.extend(members.iter().map(|m| m.company_id.clone()));
        }
        self.require_owners(deps, &person, &companies, request_id)
            .await?;
        let accessible = self
            .visible(deps, &person, &companies, "groups.update", request_id)
            .await?;
        self.execute(
            deps,
            &GroupsUpdate { accessible },
            presented,
            request_id,
            input,
        )
        .await
    }

    /// Checks that `doc` is a PNG or JPEG this company stores, within the logo
    /// size limit. The command's own guard then proves it again in the batch.
    async fn check_logo_document(
        &self,
        deps: &Deps,
        doc: &str,
        request_id: &str,
    ) -> Result<(), Problem> {
        let stored = ledgers_documents::store::get(deps.db.as_ref(), &self.company_id, doc)
            .await
            .map_err(|err| database(err, request_id))?;
        let Some(stored) = stored else {
            return Err(invalid(
                "the logo must be a document this company has stored",
                request_id,
            ));
        };
        ledgers_documents::logo::check_logo(&stored.mime, stored.size_bytes)
            .map_err(|err| invalid(err.to_string(), request_id))
    }

    /// `agreements.create`: the request's company is a party, and the person
    /// may create agreements in both companies.
    async fn agreements_create(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        mut input: Value,
    ) -> Result<Value, Problem> {
        let person = self
            .person_id(deps, presented.clone(), request_id, None)
            .await?;
        let parsed: AgreementsCreateInput = parse(&input, request_id)?;
        if self.company_id != parsed.licensor_company_id
            && self.company_id != parsed.licensee_company_id
        {
            return Err(not_found(request_id));
        }
        self.require_roles(
            deps,
            &person,
            &[&parsed.licensor_company_id, &parsed.licensee_company_id],
            "agreements.create",
            request_id,
        )
        .await?;
        fill_server_fields(&mut input, deps, "id", &person);
        self.execute(deps, &AgreementsCreate, presented, request_id, input)
            .await
    }

    /// `agreements.list`: the company's own side of each agreement, with the
    /// counterparties the person may see.
    async fn agreements_list(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
    ) -> Result<Value, Problem> {
        let person = self
            .person_id(deps, presented.clone(), request_id, None)
            .await?;
        let rows = gstore::agreements_of_company(deps.db.as_ref(), &self.company_id)
            .await
            .map_err(|err| database(err, request_id))?;
        let mut candidates: BTreeSet<String> =
            rows.iter().map(|r| r.counterparty_id.clone()).collect();
        candidates.insert(self.company_id.clone());
        let accessible = self
            .visible(deps, &person, &candidates, "agreements.list", request_id)
            .await?;
        self.execute(
            deps,
            &AgreementsList { accessible },
            presented,
            request_id,
            json!({}),
        )
        .await
    }

    /// `agreements.get`: the request's company's side and its periods.
    async fn agreements_get(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let person = self
            .person_id(deps, presented.clone(), request_id, None)
            .await?;
        let parsed: AgreementIdInput = parse(&input, request_id)?;
        let (licensor, licensee) = self
            .agreement_in_company(deps, &parsed.id, request_id)
            .await?;
        let candidates = BTreeSet::from([licensor, licensee]);
        let accessible = self
            .visible(deps, &person, &candidates, "agreements.get", request_id)
            .await?;
        self.execute(
            deps,
            &AgreementsGet { accessible },
            presented,
            request_id,
            input,
        )
        .await
    }

    /// `agreements.update`: both companies must allow the update.
    async fn agreements_update(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let person = self
            .person_id(deps, presented.clone(), request_id, None)
            .await?;
        let parsed: AgreementsUpdateInput = parse(&input, request_id)?;
        let (licensor, licensee) = self
            .agreement_in_company(deps, &parsed.id, request_id)
            .await?;
        self.require_roles(
            deps,
            &person,
            &[&licensor, &licensee],
            "agreements.update",
            request_id,
        )
        .await?;
        let candidates = BTreeSet::from([licensor, licensee]);
        let accessible = self
            .visible(deps, &person, &candidates, "agreements.update", request_id)
            .await?;
        self.execute(
            deps,
            &AgreementsUpdate { accessible },
            presented,
            request_id,
            input,
        )
        .await
    }

    /// `agreements.terminate`: both companies must allow it.
    async fn agreements_terminate(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let person = self
            .person_id(deps, presented.clone(), request_id, None)
            .await?;
        let parsed: AgreementIdInput = parse(&input, request_id)?;
        let (licensor, licensee) = self
            .agreement_in_company(deps, &parsed.id, request_id)
            .await?;
        self.require_roles(
            deps,
            &person,
            &[&licensor, &licensee],
            "agreements.terminate",
            request_id,
        )
        .await?;
        let candidates = BTreeSet::from([licensor, licensee]);
        let accessible = self
            .visible(
                deps,
                &person,
                &candidates,
                "agreements.terminate",
                request_id,
            )
            .await?;
        self.execute(
            deps,
            &AgreementsTerminate { accessible },
            presented,
            request_id,
            input,
        )
        .await
    }

    /// `agreements.run_period`: both companies must allow the drafts.
    async fn agreements_run_period(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let person = self
            .person_id(deps, presented.clone(), request_id, None)
            .await?;
        let parsed: RunPeriodInput = parse(&input, request_id)?;
        let (licensor, licensee) = self
            .agreement_in_company(deps, &parsed.id, request_id)
            .await?;
        self.require_roles(
            deps,
            &person,
            &[&licensor, &licensee],
            "agreements.run_period",
            request_id,
        )
        .await?;
        self.execute(deps, &AgreementsRunPeriod, presented, request_id, input)
            .await
    }

    /// `agreements.post_period` and `agreements.settle_period`: each period
    /// posts (or settles) on both sides or on neither, in the order given.
    async fn agreements_period_action(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
        post: bool,
    ) -> Result<Value, Problem> {
        let person = self
            .person_id(deps, presented.clone(), request_id, None)
            .await?;
        let parsed: PeriodsInput = parse(&input, request_id)?;
        let periods =
            check_periods(&parsed.periods).map_err(|err| err.to_problem().instance(request_id))?;
        let command = if post {
            "agreements.post_period"
        } else {
            "agreements.settle_period"
        };
        let (licensor, licensee) = self
            .agreement_in_company(deps, &parsed.id, request_id)
            .await?;
        self.require_roles(deps, &person, &[&licensor, &licensee], command, request_id)
            .await?;
        let call_id = deps.id_gen.ulid();
        let now = rfc3339_utc(deps.clock.now());
        let sides = DispatchSides {
            dispatch: self,
            deps,
            presented: presented.clone(),
            request_id,
        };
        let mut out = Vec::new();
        for period in &periods {
            let result = if post {
                orchestrate::post_period(
                    deps.db.as_ref(),
                    &sides,
                    &parsed.id,
                    period,
                    &call_id,
                    &now,
                )
                .await
            } else {
                orchestrate::settle_period(
                    deps.db.as_ref(),
                    &sides,
                    &parsed.id,
                    period,
                    &call_id,
                    &now,
                )
                .await
            }
            .map_err(|err| err.to_problem().instance(request_id))?;
            out.push(PeriodPair {
                period: result.period,
                docs: result.docs,
            });
        }
        serde_json::to_value(PeriodsOutput {
            agreement_id: parsed.id,
            periods: out,
        })
        .map_err(|err| invalid(err.to_string(), request_id))
    }

    /// `reports.intercompany_reconciliation`: the agreements whose both
    /// companies the person may see. A group the person sees no member of is
    /// not found.
    async fn reconciliation(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let person = self
            .person_id(deps, presented.clone(), request_id, None)
            .await?;
        let parsed: ReconciliationInput = parse(&input, request_id)?;
        let members = gstore::members_of(deps.db.as_ref(), &parsed.id)
            .await
            .map_err(|err| database(err, request_id))?;
        let candidates: BTreeSet<String> = members.into_iter().map(|m| m.company_id).collect();
        let accessible = self
            .visible(
                deps,
                &person,
                &candidates,
                "reports.intercompany_reconciliation",
                request_id,
            )
            .await?;
        if accessible.is_empty() {
            return Err(not_found(request_id));
        }
        self.execute(
            deps,
            &ReconciliationReport { accessible },
            presented,
            request_id,
            input,
        )
        .await
    }

    /// `reports.group_view`: the members the person may see, side by side.
    async fn group_view(
        &self,
        deps: &Deps,
        presented: Option<Presented>,
        request_id: &str,
        input: Value,
    ) -> Result<Value, Problem> {
        let person = self
            .person_id(deps, presented.clone(), request_id, None)
            .await?;
        let parsed: GroupViewInput = parse(&input, request_id)?;
        let members = gstore::members_of(deps.db.as_ref(), &parsed.id)
            .await
            .map_err(|err| database(err, request_id))?;
        let candidates: BTreeSet<String> = members.into_iter().map(|m| m.company_id).collect();
        let accessible = self
            .visible(deps, &person, &candidates, "reports.group_view", request_id)
            .await?;
        if accessible.is_empty() {
            return Err(not_found(request_id));
        }
        self.execute(
            deps,
            &GroupViewReport { accessible },
            presented,
            request_id,
            input,
        )
        .await
    }
}
