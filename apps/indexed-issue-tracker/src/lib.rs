//! Canonical example for [`IndexedMap`] — an issue tracker whose list views
//! are index seeks instead of scans over every issue.
//!
//! # The shape this collection is for
//!
//! Nearly every list method an app writes reads the whole map, filters by a
//! field, sorts, and counts. On an `UnorderedMap` each of those is `O(n)` in
//! every issue ever filed, however few it returns. Here the value type declares
//! what it is looked up by, and the reads below cost what they return:
//!
//! * `list_by_status("open", ..)` — newest first, one page — seeks the compound
//!   `(status, created_at)` index and walks back from its end;
//! * `list_by_label` and `list_by_assignee` — a multi-valued field and an
//!   optional one — seek their own indexes;
//! * `status_counts` counts index rows and loads no issue at all.
//!
//! # What it costs, and what it does not
//!
//! The indexes are node-local: nothing extra is synced, and the stored bytes
//! are an `UnorderedMap`'s. A change that arrives from a peer is applied without
//! the indexes being told, so the first query after it rebuilds them — the
//! price of maintaining an index from inside the app. `workflows/` drives that
//! across two real nodes.

use calimero_sdk::abi::AbiType;
use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{IndexedMap, LwwRegister};
use thiserror::Error;

/// The statuses an issue moves through, in board order.
const STATUSES: [&str; 3] = ["open", "in_progress", "closed"];

/// One issue, and what it is looked up by.
///
/// Each field is an `LwwRegister` because an entry's value must merge; the
/// derive merges field by field, so a concurrent reassign and relabel both
/// survive.
#[derive(BorshSerialize, BorshDeserialize, AbiType, app::Mergeable, app::Indexed)]
#[borsh(crate = "calimero_sdk::borsh")]
#[index(status_created(status, created_at))]
pub struct Issue {
    #[index]
    pub status: LwwRegister<String>,
    /// `None` leaves the issue out of the `assignee` index entirely.
    #[index]
    pub assignee: LwwRegister<Option<String>>,
    /// One `labels` index row per label.
    #[index]
    pub labels: LwwRegister<Vec<String>>,
    pub created_at: LwwRegister<u64>,
    pub title: LwwRegister<String>,
}

#[app::state(emits = for<'a> Event<'a>)]
pub struct IssueTracker {
    /// Issue id -> issue.
    issues: IndexedMap<String, Issue>,
}

#[app::event]
pub enum Event<'a> {
    Opened { id: &'a str },
    Changed { id: &'a str },
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("issue {0} already exists")]
    Exists(&'a str),
    #[error("no issue {0}")]
    NotFound(&'a str),
    #[error("unknown status {0}")]
    UnknownStatus(&'a str),
}

/// An issue as a client sees it.
#[derive(Debug, Serialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct IssueView {
    pub id: String,
    pub title: String,
    pub status: String,
    pub assignee: Option<String>,
    pub labels: Vec<String>,
    pub created_at: u64,
}

/// How many issues one status holds.
#[derive(Debug, Serialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct StatusCount {
    pub status: String,
    pub count: u64,
}

fn view_of(id: String, issue: &Issue) -> IssueView {
    IssueView {
        id,
        title: issue.title.get().clone(),
        status: issue.status.get().clone(),
        assignee: issue.assignee.get().clone(),
        labels: issue.labels.get().clone(),
        created_at: *issue.created_at.get(),
    }
}

fn views(entries: Vec<(String, Issue)>) -> Vec<IssueView> {
    entries
        .into_iter()
        .map(|(id, issue)| view_of(id, &issue))
        .collect()
}

#[app::logic]
impl IssueTracker {
    #[app::init]
    pub fn init() -> IssueTracker {
        IssueTracker {
            issues: IndexedMap::new(),
        }
    }

    /// File an issue. `created_at` is the client's clock, as a message
    /// timestamp is: it orders the issue within its status.
    pub fn open(
        &mut self,
        id: String,
        title: String,
        labels: Vec<String>,
        created_at: u64,
    ) -> app::Result<()> {
        if self.issues.contains(&id)? {
            app::bail!(Error::Exists(&id));
        }
        let issue = Issue {
            status: LwwRegister::new(STATUSES[0].to_owned()),
            assignee: LwwRegister::new(None),
            labels: LwwRegister::new(labels),
            created_at: LwwRegister::new(created_at),
            title: LwwRegister::new(title),
        };
        let _ = self.issues.insert(id.clone(), issue)?;
        app::emit!(Event::Opened { id: &id });
        Ok(())
    }

    /// Move an issue to another status. `update` keeps every index in step.
    pub fn set_status(&mut self, id: String, status: String) -> app::Result<()> {
        if !STATUSES.contains(&status.as_str()) {
            app::bail!(Error::UnknownStatus(&status));
        }
        if self
            .issues
            .update(&id, |issue| issue.status.set(status))?
            .is_none()
        {
            app::bail!(Error::NotFound(&id));
        }
        app::emit!(Event::Changed { id: &id });
        Ok(())
    }

    /// Assign an issue, or unassign it with `None`.
    pub fn assign(&mut self, id: String, assignee: Option<String>) -> app::Result<()> {
        if self
            .issues
            .update(&id, |issue| issue.assignee.set(assignee))?
            .is_none()
        {
            app::bail!(Error::NotFound(&id));
        }
        app::emit!(Event::Changed { id: &id });
        Ok(())
    }

    pub fn get(&self, id: String) -> app::Result<Option<IssueView>> {
        Ok(self.issues.get(&id)?.map(|issue| view_of(id, &issue)))
    }

    /// One page of a status column, newest first.
    pub fn list_by_status(
        &self,
        status: String,
        offset: usize,
        limit: usize,
    ) -> app::Result<Vec<IssueView>> {
        Ok(views(
            self.issues
                .query("status_created")
                .eq(&status)
                .desc()
                .skip(offset)
                .limit(limit)
                .entries()?,
        ))
    }

    /// Issues in one status created within `[from, to)`, oldest first.
    pub fn list_created_between(
        &self,
        status: String,
        from: u64,
        to: u64,
    ) -> app::Result<Vec<IssueView>> {
        Ok(views(
            self.issues
                .query("status_created")
                .eq(&status)
                .range(from..to)
                .entries()?,
        ))
    }

    pub fn list_by_label(&self, label: String) -> app::Result<Vec<IssueView>> {
        Ok(views(self.issues.query("labels").eq(&label).entries()?))
    }

    pub fn list_by_assignee(&self, assignee: String) -> app::Result<Vec<IssueView>> {
        Ok(views(
            self.issues
                .query("assignee")
                .eq(&Some(assignee))
                .entries()?,
        ))
    }

    /// Issues per status — index rows counted, no issue loaded.
    pub fn status_counts(&self) -> app::Result<Vec<StatusCount>> {
        let mut counts = Vec::with_capacity(STATUSES.len());
        for status in STATUSES {
            counts.push(StatusCount {
                status: status.to_owned(),
                count: self.issues.query("status").eq(status).count()? as u64,
            });
        }
        Ok(counts)
    }

    pub fn issue_count(&self) -> app::Result<u64> {
        Ok(self.issues.len()? as u64)
    }
}

#[cfg(test)]
mod tests {
    use calimero_sdk::testing::TestHost;

    use super::*;

    fn tracker() -> TestHost<IssueTracker> {
        let mut app = TestHost::new(IssueTracker::init);
        for (id, labels, at) in [
            ("t1", vec!["bug", "ui"], 10),
            ("t2", vec!["bug"], 20),
            ("t3", vec!["feature"], 30),
            ("t4", vec![], 40),
        ] {
            app.call(|s| {
                s.open(
                    id.into(),
                    format!("issue {id}"),
                    labels.into_iter().map(str::to_owned).collect(),
                    at,
                )
            })
            .expect("open");
        }
        app
    }

    fn ids(views: Vec<IssueView>) -> Vec<String> {
        views.into_iter().map(|v| v.id).collect()
    }

    #[test]
    fn a_status_column_pages_newest_first() {
        let app = tracker();
        let page = |offset, limit| {
            ids(app
                .view(|s| s.list_by_status("open".into(), offset, limit))
                .expect("list"))
        };
        assert_eq!(page(0, 2), ["t4", "t3"]);
        assert_eq!(page(2, 2), ["t2", "t1"]);
        assert!(page(4, 2).is_empty());
    }

    #[test]
    fn moving_an_issue_moves_it_between_columns_and_counts() {
        let mut app = tracker();
        app.call(|s| s.set_status("t3".into(), "closed".into()))
            .expect("close");

        let open = ids(app
            .view(|s| s.list_by_status("open".into(), 0, 10))
            .expect("list"));
        assert_eq!(open, ["t4", "t2", "t1"]);
        let closed = ids(app
            .view(|s| s.list_by_status("closed".into(), 0, 10))
            .expect("list"));
        assert_eq!(closed, ["t3"]);

        let counts: Vec<(String, u64)> = app
            .view(|s| s.status_counts())
            .expect("counts")
            .into_iter()
            .map(|c| (c.status, c.count))
            .collect();
        assert_eq!(
            counts,
            [
                ("open".to_owned(), 3),
                ("in_progress".to_owned(), 0),
                ("closed".to_owned(), 1)
            ]
        );
    }

    #[test]
    fn labels_assignees_and_time_ranges_are_seeks() {
        let mut app = tracker();
        app.call(|s| s.assign("t2".into(), Some("alice".into())))
            .expect("assign");
        app.call(|s| s.assign("t4".into(), Some("alice".into())))
            .expect("assign");
        app.call(|s| s.assign("t4".into(), None)).expect("unassign");

        // Equal values come back in entry-id order, which is arbitrary.
        let mut bugs = ids(app.view(|s| s.list_by_label("bug".into())).expect("label"));
        bugs.sort();
        assert_eq!(bugs, ["t1", "t2"]);
        assert_eq!(
            ids(app
                .view(|s| s.list_by_assignee("alice".into()))
                .expect("assignee")),
            ["t2"]
        );
        assert_eq!(
            ids(app
                .view(|s| s.list_created_between("open".into(), 20, 40))
                .expect("range")),
            ["t2", "t3"]
        );
    }

    #[test]
    fn misuse_is_refused() {
        let mut app = tracker();
        assert!(app
            .call(|s| s.open("t1".into(), "again".into(), vec![], 1))
            .is_err());
        assert!(app
            .call(|s| s.set_status("t1".into(), "someday".into()))
            .is_err());
        assert!(app
            .call(|s| s.set_status("nope".into(), "closed".into()))
            .is_err());
    }
}
