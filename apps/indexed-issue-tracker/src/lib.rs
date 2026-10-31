//! An issue tracker whose list views are index seeks instead of scans over every issue.
//!
//! The v1 version was the canonical example for `IndexedMap` + `#[derive(app::Indexed)]`; on
//! the component SDK a `Table` declares the same lookups and the reads cost what they return:
//!
//! * `listByStatus("open", ..)`, newest first, one page: a reverse seek of the compound
//!   `statusCreated = (status, createdAt)` index;
//! * `listCreatedBetween`: the same index, bounded by `createdAt`;
//! * `listByAssignee`: the `byAssignee` index (an unassigned issue is indexed under `null`,
//!   which no lookup by name reaches);
//! * `listByLabel`: labels are a list, so each label of an issue is a row of its own in the
//!   `labels` table, keyed `(issueId, label)` and indexed `byLabel`;
//! * `statusCounts`: the table's maintained `byStatus` count, no issue loaded.
//!
//! Every change goes through `Table::update`, which keeps each index and count in step. The
//! indexes live in the app's state like the issues, so a replica never rebuilds them.
//!
//! Access: any org member (or above) files, moves and assigns issues and reads the lists;
//! guests and outsiders get `Forbidden`.

use calimero_app_sdk::prelude::*;

/// The statuses an issue moves through, in board order.
pub const STATUSES: [&str; 3] = ["open", "in_progress", "closed"];

/// The most issues one list call returns.
pub const MAX_PAGE: u32 = 1000;

/// One issue, and what it is looked up by. Its labels are rows of [`Label`].
#[derive(Record)]
#[table(
    key = "id",
    index(status_created = ["status", "created_at"]),
    index(by_assignee = ["assignee"]),
    count(by_status = ["status"]),
)]
pub struct Issue {
    pub id: String,
    pub title: String,
    pub status: String,
    /// `None` leaves the issue out of every lookup by assignee.
    pub assignee: Option<String>,
    /// The client's clock, as a message timestamp is: it orders the issue within its status.
    pub created_at: u64,
}

/// One label of one issue: the multi-valued index `labels` was in v1.
#[derive(Record)]
#[table(
    key = ["issue_id", "label"],
    index(by_label = ["label", "issue_id"]),
    index(by_issue = ["issue_id", "label"]),
)]
pub struct Label {
    pub issue_id: String,
    pub label: String,
}

/// An issue as a client sees it, labels included. (A list field cannot be a `Record`'s.)
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueView {
    pub id: String,
    pub title: String,
    pub status: String,
    pub assignee: Option<String>,
    pub labels: Vec<String>,
    #[serde(with = "calimero_app_sdk::json::u64_str")]
    pub created_at: u64,
}

/// How many issues one status holds.
#[derive(Record)]
#[table(key = "status")]
pub struct StatusCount {
    pub status: String,
    pub count: u64,
}

#[app::state(version = 1)]
pub struct IssueTracker {
    issues: Table<Issue>,
    labels: Table<Label>,
}

#[app::error]
pub enum Error {
    Exists { id: String },
    NotFound { id: String },
    UnknownStatus { status: String },
}

#[app::event]
pub enum Event {
    Opened { id: String },
    Changed { id: String },
}

#[app::logic]
impl IssueTracker {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// Files an issue, `open`. `createdAt` orders it within its status.
    pub fn open(
        &mut self,
        id: String,
        title: String,
        labels: Vec<String>,
        created_at: u64,
    ) -> app::Result<()> {
        guards::require_member()?;
        if self.issues.contains(&id)? {
            app::bail!(Error::Exists { id });
        }
        let mut labels = labels;
        labels.sort();
        labels.dedup();
        for label in &labels {
            self.labels.insert(Label {
                label: label.clone(),
                issue_id: id.clone(),
            })?;
        }
        self.issues.insert(Issue {
            id: id.clone(),
            title,
            status: STATUSES[0].to_owned(),
            assignee: None,
            created_at,
        })?;
        env::emit(Event::Opened { id }, Audience::Org)?;
        Ok(())
    }

    /// Moves an issue to another status; its index rows and the counts follow.
    pub fn set_status(&mut self, id: String, status: String) -> app::Result<()> {
        guards::require_member()?;
        if !STATUSES.contains(&status.as_str()) {
            app::bail!(Error::UnknownStatus { status });
        }
        if !self.issues.update(&id, |i| i.status = status)? {
            app::bail!(Error::NotFound { id });
        }
        env::emit(Event::Changed { id }, Audience::Org)?;
        Ok(())
    }

    /// Assigns an issue, or unassigns it with `null`.
    pub fn assign(&mut self, id: String, assignee: Option<String>) -> app::Result<()> {
        guards::require_member()?;
        if !self.issues.update(&id, |i| i.assignee = assignee)? {
            app::bail!(Error::NotFound { id });
        }
        env::emit(Event::Changed { id }, Audience::Org)?;
        Ok(())
    }

    #[app::view]
    pub fn get(&self, id: String) -> app::Result<Option<IssueView>> {
        guards::require_member()?;
        self.issues.get(&id)?.map(|i| self.view_of(i)).transpose()
    }

    /// One page of a status column, newest first: `limit` issues after the first `offset`
    /// (together at most `MAX_PAGE`).
    #[app::view]
    pub fn list_by_status(
        &self,
        status: String,
        offset: u32,
        limit: u32,
    ) -> app::Result<Vec<IssueView>> {
        guards::require_member()?;
        let read = offset.saturating_add(limit).min(MAX_PAGE);
        let page = self.issues.find(
            "statusCreated",
            &[KeyPart::Str(status)],
            FindOpts::default().reverse().limit(read),
        )?;
        self.views(page.items.into_iter().skip(offset as usize))
    }

    /// Issues in one status created within `[from, to)`, oldest first (at most `MAX_PAGE`).
    #[app::view]
    pub fn list_created_between(
        &self,
        status: String,
        from: u64,
        to: u64,
    ) -> app::Result<Vec<IssueView>> {
        guards::require_member()?;
        let page = self.issues.find(
            "statusCreated",
            &[KeyPart::Str(status)],
            FindOpts::default()
                .from(vec![KeyPart::Int(i128::from(from))])
                .to(vec![KeyPart::Int(i128::from(to))])
                .limit(MAX_PAGE),
        )?;
        self.views(page.items)
    }

    /// The issues carrying `label`, by id (at most `MAX_PAGE`).
    #[app::view]
    pub fn list_by_label(&self, label: String) -> app::Result<Vec<IssueView>> {
        guards::require_member()?;
        let rows = self.labels.find(
            "byLabel",
            &[KeyPart::Str(label)],
            FindOpts::default().limit(MAX_PAGE),
        )?;
        let mut out = Vec::with_capacity(rows.items.len());
        for row in rows.items {
            if let Some(issue) = self.issues.get(&row.issue_id)? {
                out.push(self.view_of(issue)?);
            }
        }
        Ok(out)
    }

    /// The issues assigned to `assignee`, by id (at most `MAX_PAGE`).
    #[app::view]
    pub fn list_by_assignee(&self, assignee: String) -> app::Result<Vec<IssueView>> {
        guards::require_member()?;
        let page = self.issues.find(
            "byAssignee",
            &[KeyPart::Str(assignee)],
            FindOpts::default().limit(MAX_PAGE),
        )?;
        self.views(page.items)
    }

    /// Issues per status, in board order: maintained counts, no issue loaded.
    #[app::view]
    pub fn status_counts(&self) -> app::Result<Vec<StatusCount>> {
        guards::require_member()?;
        let mut out = Vec::with_capacity(STATUSES.len());
        for status in STATUSES {
            out.push(StatusCount {
                status: status.to_owned(),
                count: self
                    .issues
                    .count("byStatus", &[KeyPart::Str(status.to_owned())])?,
            });
        }
        Ok(out)
    }

    /// Every issue filed.
    #[app::view]
    pub fn issue_count(&self) -> app::Result<u64> {
        guards::require_member()?;
        Ok(self.issues.count("byStatus", &[])?)
    }

    /// The issue with its labels.
    fn view_of(&self, issue: Issue) -> app::Result<IssueView> {
        let rows = self
            .labels
            .find(
                "byIssue",
                &[KeyPart::Str(issue.id.clone())],
                FindOpts::default().limit(MAX_PAGE),
            )?
            .items
            .into_iter()
            .map(|l| l.label)
            .collect();
        Ok(IssueView {
            id: issue.id,
            title: issue.title,
            status: issue.status,
            assignee: issue.assignee,
            labels: rows,
            created_at: issue.created_at,
        })
    }

    fn views(&self, issues: impl IntoIterator<Item = Issue>) -> app::Result<Vec<IssueView>> {
        issues.into_iter().map(|i| self.view_of(i)).collect()
    }
}
