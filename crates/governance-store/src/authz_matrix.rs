//! Authorization matrix: every gated operation run against every actor state.
//! A new `GatedOp` needs a table and a row; the coverage tests fail without both.
//! Which signed op variants have no row is classified in `calimero-governance-types` (`authz_classification`).

mod world;

#[cfg(test)]
mod rows;

use std::collections::BTreeMap;
use std::fmt::Write as _;

pub use self::world::{Actor, World};

macro_rules! closed_set {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($(#[$vmeta:meta])* $variant:ident => $label:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        $vis enum $name {
            $($(#[$vmeta])* $variant),+
        }

        impl $name {
            /// Every variant, generated from the declaration so none can be left out.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub const fn label(self) -> &'static str {
                match self {
                    $($name::$variant => $label),+
                }
            }
        }
    };
}

closed_set! {
    /// What an actor is to the world's subject group, reached by signed ops only.
    pub enum ActorState {
        Owner => "owner",
        NamespaceAdmin => "namespace admin",
        DirectAdmin => "direct admin",
        DirectMember => "direct member",
        InheritedAdmin => "inherited admin",
        InheritedMember => "inherited member",
        Kicked => "kicked",
        Left => "left",
        DenyListed => "deny-listed",
        ReadmittedAfterKick => "readmitted",
        RevokedDevice => "revoked device",
        DescopedDevice => "descoped device",
        SecondDevice => "second device",
        OtherNamespaceMember => "other namespace",
        NonMember => "non-member",
    }
}

closed_set! {
    /// An operation that grants something and so must be gated on the actor.
    pub enum GatedOp {
        GroupKeyPull => "key pull (subgroup)",
        NamespaceKeyPull => "key pull (namespace)",
        OpenChainKeyPull => "key pull (open chain)",
        NamespaceJoinKey => "namespace join key",
        OpenSubgroupJoinKey => "open-subgroup join key",
        AcceptNamespaceJoinKey => "accept ns join key",
        AcceptOpenSubgroupKey => "accept subgroup key",
        AcceptRecoveredKey => "accept recovered key",
        DeviceLink => "device link",
        DeviceRevoke => "device revoke",
        DeviceDescope => "device descope",
        ForeignDescope => "descope another's device",
        RelayAuthor => "relay author",
        SseSubscribe => "sse subscribe",
        WsSubscribe => "ws subscribe",
        ListSubgroups => "list subgroups",
        SubgroupInScope => "subgroup in scope",
    }
}

/// Live members of the subject, by any path, on a live device of theirs.
pub const SUBJECT_MEMBERS: &[ActorState] = &[
    ActorState::Owner,
    ActorState::DirectAdmin,
    ActorState::DirectMember,
    ActorState::InheritedAdmin,
    ActorState::InheritedMember,
    ActorState::ReadmittedAfterKick,
    ActorState::SecondDevice,
];

/// Accounts the namespace admitted and has not removed, on a live device.
pub const NAMESPACE_MEMBERS: &[ActorState] = &[
    ActorState::Owner,
    ActorState::NamespaceAdmin,
    ActorState::DirectAdmin,
    ActorState::DirectMember,
    ActorState::InheritedAdmin,
    ActorState::InheritedMember,
    ActorState::Kicked,
    ActorState::Left,
    ActorState::ReadmittedAfterKick,
    ActorState::SecondDevice,
];

/// The crate whose tests hold a `GatedOp`'s rows and table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Home {
    GovernanceStore,
    Context,
    Node,
    Server,
}

impl GatedOp {
    pub const fn home(self) -> Home {
        match self {
            Self::GroupKeyPull
            | Self::NamespaceKeyPull
            | Self::OpenChainKeyPull
            | Self::DeviceLink
            | Self::DeviceRevoke
            | Self::DeviceDescope
            | Self::ForeignDescope
            | Self::RelayAuthor => Home::GovernanceStore,
            Self::AcceptNamespaceJoinKey => Home::Context,
            Self::NamespaceJoinKey
            | Self::OpenSubgroupJoinKey
            | Self::AcceptOpenSubgroupKey
            | Self::AcceptRecoveredKey => Home::Node,
            Self::SseSubscribe
            | Self::WsSubscribe
            | Self::ListSubgroups
            | Self::SubgroupInScope => Home::Server,
        }
    }
}

/// What an operation did for an actor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Allow,
    Refuse,
}

impl From<bool> for Outcome {
    fn from(allowed: bool) -> Self {
        if allowed {
            Self::Allow
        } else {
            Self::Refuse
        }
    }
}

/// A gated operation run through its real entry point by one actor.
pub type Row = (GatedOp, fn(&World, &Actor) -> Outcome);

/// Run every row for every actor state of the shared world.
pub fn observe(rows: &[Row]) -> Observed {
    let world = World::shared();
    let mut observed = Observed::default();
    for (op, row) in rows {
        for state in ActorState::ALL {
            observed.record(*op, *state, row(world, world.actor(*state)));
        }
    }
    observed
}

/// What an operation must do for an actor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    Allow,
    Refuse,
    /// Should refuse but allows today. Reported; fails once it refuses, so the
    /// cell is flipped to `Refuse` and cannot silently reopen.
    ExpectedGap,
}

/// The expected outcomes of one operation: anything not listed refuses.
pub struct OpTable {
    pub op: GatedOp,
    pub allow: &'static [ActorState],
    pub gap: &'static [ActorState],
}

impl OpTable {
    pub fn expect(&self, state: ActorState) -> Expect {
        if self.allow.contains(&state) {
            Expect::Allow
        } else if self.gap.contains(&state) {
            Expect::ExpectedGap
        } else {
            Expect::Refuse
        }
    }
}

/// What the real entry points did, one cell per operation and state.
#[derive(Default)]
pub struct Observed(BTreeMap<(GatedOp, ActorState), Outcome>);

impl Observed {
    pub fn record(&mut self, op: GatedOp, state: ActorState, outcome: Outcome) {
        let previous = self.0.insert((op, state), outcome);
        assert!(previous.is_none(), "{op:?} / {state:?} observed twice");
    }

    pub fn get(&self, op: GatedOp, state: ActorState) -> Option<Outcome> {
        self.0.get(&(op, state)).copied()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Holds(Outcome),
    Gap,
    GapClosed,
    Wrong(Expect, Option<Outcome>),
}

fn judge(expect: Expect, observed: Option<Outcome>) -> Verdict {
    match (expect, observed) {
        (Expect::Allow, Some(Outcome::Allow)) => Verdict::Holds(Outcome::Allow),
        (Expect::Refuse, Some(Outcome::Refuse)) => Verdict::Holds(Outcome::Refuse),
        (Expect::ExpectedGap, Some(Outcome::Allow)) => Verdict::Gap,
        (Expect::ExpectedGap, Some(Outcome::Refuse)) => Verdict::GapClosed,
        (expect, observed) => Verdict::Wrong(expect, observed),
    }
}

fn cell(verdict: Verdict) -> String {
    match verdict {
        Verdict::Holds(Outcome::Allow) => "allow".to_owned(),
        Verdict::Holds(Outcome::Refuse) => "refuse".to_owned(),
        Verdict::Gap => "gap".to_owned(),
        Verdict::GapClosed => "!! gap closed, flip to refuse".to_owned(),
        Verdict::Wrong(expect, got) => {
            let want = match expect {
                Expect::Allow => "allow",
                Expect::Refuse => "refuse",
                Expect::ExpectedGap => "gap",
            };
            let got = match got {
                Some(Outcome::Allow) => "allow",
                Some(Outcome::Refuse) => "refuse",
                None => "no row",
            };
            format!("!! want {want}, got {got}")
        }
    }
}

/// The whole grid, states down and operations across, differing cells marked `!!`.
fn render(group: &str, tables: &[OpTable], observed: &Observed) -> (String, usize, Vec<String>) {
    let mut cells = Vec::new();
    let mut wrong = 0;
    let mut gaps = Vec::new();
    for state in ActorState::ALL {
        let mut line = Vec::new();
        for table in tables {
            let verdict = judge(table.expect(*state), observed.get(table.op, *state));
            match verdict {
                Verdict::Wrong(..) | Verdict::GapClosed => wrong += 1,
                Verdict::Gap => gaps.push(format!("{} / {}", table.op.label(), state.label())),
                Verdict::Holds(_) => {}
            }
            line.push(cell(verdict));
        }
        cells.push(line);
    }

    let first = ActorState::ALL
        .iter()
        .map(|s| s.label().len())
        .max()
        .unwrap_or(0);
    let widths: Vec<usize> = tables
        .iter()
        .enumerate()
        .map(|(i, t)| {
            cells
                .iter()
                .map(|row| row[i].len())
                .chain([t.op.label().len()])
                .max()
                .unwrap_or(0)
        })
        .collect();

    let mut out = String::new();
    let _ = writeln!(out, "authorization matrix: {group}");
    let _ = write!(out, "{:first$}", "");
    for (table, width) in tables.iter().zip(&widths) {
        let _ = write!(out, " | {:width$}", table.op.label());
    }
    out.push('\n');
    for (state, line) in ActorState::ALL.iter().zip(&cells) {
        let _ = write!(out, "{:first$}", state.label());
        for (text, width) in line.iter().zip(&widths) {
            let _ = write!(out, " | {text:width$}");
        }
        out.push('\n');
    }
    (out, wrong, gaps)
}

/// Compare observed outcomes with the tables. Prints known gaps; panics with the
/// whole grid when any cell differs or a known gap has closed.
pub fn assert_matches(group: &str, tables: &[OpTable], observed: &Observed) {
    let (grid, wrong, gaps) = render(group, tables, observed);
    if !gaps.is_empty() {
        eprintln!(
            "authorization matrix {group}: {} known gap(s), allowed today but should refuse:",
            gaps.len()
        );
        for gap in &gaps {
            eprintln!("  {gap}");
        }
    }
    assert!(
        wrong == 0,
        "{wrong} cell(s) differ from the table\n{grid}\nlegend: gap = allowed today but \
         should refuse; a closed gap must be flipped to refuse in the table"
    );
}

/// Fail unless every operation homed in `home` has one row and one table, and
/// no table lists a state as both allowed and a gap.
pub fn assert_covered(home: Home, rows: &[GatedOp], tables: &[OpTable]) {
    for table in tables {
        assert_eq!(
            table.op.home(),
            home,
            "{:?} has a table outside its home crate",
            table.op
        );
        assert_eq!(
            tables.iter().filter(|t| t.op == table.op).count(),
            1,
            "{:?} has more than one table",
            table.op
        );
        for state in table.allow {
            assert!(
                !table.gap.contains(state),
                "{:?} lists {state:?} as both allow and gap",
                table.op
            );
        }
    }
    for op in rows {
        assert_eq!(op.home(), home, "{op:?} has a row outside its home crate");
        assert_eq!(
            rows.iter().filter(|r| *r == op).count(),
            1,
            "{op:?} has more than one row"
        );
    }
    for op in GatedOp::ALL.iter().filter(|op| op.home() == home) {
        assert!(rows.contains(op), "{op:?} has no row in {home:?}");
        assert!(
            tables.iter().any(|t| t.op == *op),
            "{op:?} has no table in {home:?}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_marks_exactly_the_cells_that_differ() {
        const TABLE: OpTable = OpTable {
            op: GatedOp::GroupKeyPull,
            allow: &[ActorState::Owner],
            gap: &[ActorState::Kicked],
        };
        let mut observed = Observed::default();
        for state in ActorState::ALL {
            let outcome = match state {
                ActorState::Owner | ActorState::Kicked => Outcome::Allow,
                ActorState::NonMember => Outcome::Allow,
                _ => Outcome::Refuse,
            };
            observed.record(GatedOp::GroupKeyPull, *state, outcome);
        }
        let (grid, wrong, gaps) = render("unit", &[TABLE], &observed);
        assert_eq!(wrong, 1, "only the non-member cell differs:\n{grid}");
        assert_eq!(gaps.len(), 1);
        assert_eq!(grid.matches("!!").count(), 1);
        assert!(grid.contains("!! want refuse, got allow"));
    }

    #[test]
    #[should_panic(expected = "observed twice")]
    fn a_cell_observed_twice_fails() {
        let mut observed = Observed::default();
        observed.record(GatedOp::GroupKeyPull, ActorState::Owner, Outcome::Allow);
        observed.record(GatedOp::GroupKeyPull, ActorState::Owner, Outcome::Allow);
    }

    #[test]
    #[should_panic(expected = "both allow and gap")]
    fn a_state_both_allowed_and_a_gap_fails() {
        const TABLE: OpTable = OpTable {
            op: GatedOp::GroupKeyPull,
            allow: &[ActorState::Owner],
            gap: &[ActorState::Owner],
        };
        assert_covered(Home::GovernanceStore, &[], &[TABLE]);
    }

    #[test]
    #[should_panic(expected = "has no row")]
    fn an_operation_without_a_row_fails() {
        assert_covered(Home::GovernanceStore, &[], &[]);
    }

    #[test]
    fn a_cell_with_no_observation_is_wrong() {
        const TABLE: OpTable = OpTable {
            op: GatedOp::GroupKeyPull,
            allow: &[],
            gap: &[],
        };
        let (_grid, wrong, _gaps) = render("unit", &[TABLE], &Observed::default());
        assert_eq!(wrong, ActorState::ALL.len());
    }

    #[test]
    fn a_closed_gap_fails_until_its_cell_is_flipped() {
        const TABLE: OpTable = OpTable {
            op: GatedOp::GroupKeyPull,
            allow: &[],
            gap: &[ActorState::Kicked],
        };
        let mut observed = Observed::default();
        for state in ActorState::ALL {
            observed.record(GatedOp::GroupKeyPull, *state, Outcome::Refuse);
        }
        let (grid, wrong, gaps) = render("unit", &[TABLE], &observed);
        assert_eq!(wrong, 1, "the closed gap is the one failure:\n{grid}");
        assert!(gaps.is_empty());
        assert!(grid.contains("!! gap closed, flip to refuse"));
    }
}
