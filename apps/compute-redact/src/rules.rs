//! The redaction rules engine: per-column rules applied to CSV streamed from a reader to a writer.
//! No clock and no randomness: the same input and policy give the same bytes.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::LazyLock;

use csv::StringRecord;
use hmac::{Hmac, KeyInit, Mac};
use regex_lite::Regex;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// Rows between progress reports.
const PROGRESS_EVERY: u64 = 1000;

static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}").unwrap());
// 13 to 19 digits, spaces or dashes between them.
static CARD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9](?:[ -]?[0-9]){12,18}").unwrap());
// An international number, or one grouped with spaces, dots, dashes or a bracketed area code.
static PHONE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\+[0-9]{8,15}|(?:\+[0-9]{1,3}[ .-]?)?(?:\([0-9]{2,4}\)[ .-]?|[0-9]{2,4}[ .-])[0-9]{3,4}[ .-]?[0-9]{3,4}")
        .unwrap()
});

pub type Result<T> = std::result::Result<T, csv::Error>;

/// What happens to a column's cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rule {
    Drop,
    Mask,
    Pseudonym,
    Scan,
}

/// Rules by column name, and the salt the pseudonyms are keyed with. Columns not named pass through.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub salt: String,
    pub columns: BTreeMap<String, Rule>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub rows: u64,
    pub cells: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preview {
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub total_rows: u64,
    pub cells: u64,
}

pub fn mask(value: &str) -> String {
    if let Some((local, domain)) = value.split_once('@') {
        let first: String = local.chars().take(1).collect();
        let tld = domain
            .rsplit_once('.')
            .map_or(String::new(), |(_, t)| format!(".{t}"));
        return format!("{first}***@***{tld}");
    }
    let digits = value.chars().filter(char::is_ascii_digit).count();
    if digits > 0 && !value.chars().any(char::is_alphabetic) {
        let mut keep = 2.min(digits);
        let mut out: Vec<char> = value.chars().rev().collect();
        for c in &mut out {
            if c.is_ascii_digit() {
                if keep > 0 {
                    keep -= 1;
                } else {
                    *c = '*';
                }
            }
        }
        return out.into_iter().rev().collect();
    }
    match value.chars().next() {
        Some(first) => format!("{first}***"),
        None => String::new(),
    }
}

pub fn pseudonym(salt: &str, value: &str) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(salt.as_bytes()).expect("hmac takes any key length");
    mac.update(value.as_bytes());
    let tag = mac.finalize().into_bytes();
    let hex: String = tag.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("P-{hex}")
}

pub fn scan(text: &str) -> String {
    let text = EMAIL.replace_all(text, "[EMAIL]");
    let text = CARD.replace_all(&text, "[CARD]");
    PHONE.replace_all(&text, "[PHONE]").into_owned()
}

/// A policy bound to one file's header: the rule of each column by position.
struct Redactor<'a> {
    salt: &'a str,
    rules: Vec<Option<Rule>>,
}

impl<'a> Redactor<'a> {
    fn new(policy: &'a Policy, header: &StringRecord) -> Self {
        let rules = header
            .iter()
            .map(|name| policy.columns.get(name).copied())
            .collect();
        Self {
            salt: &policy.salt,
            rules,
        }
    }

    /// The redacted row and how many of its cells changed.
    fn row(&self, record: &StringRecord) -> (StringRecord, u64) {
        let mut changed = 0;
        let out = record
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let new = match self.rules.get(i).copied().flatten() {
                    None => return cell.to_owned(),
                    Some(Rule::Drop) => String::new(),
                    Some(Rule::Mask) => mask(cell),
                    Some(Rule::Pseudonym) if cell.is_empty() => String::new(),
                    Some(Rule::Pseudonym) => pseudonym(self.salt, cell),
                    Some(Rule::Scan) => scan(cell),
                };
                changed += u64::from(new != cell);
                new
            })
            .collect();
        (out, changed)
    }
}

fn reader<R: Read>(input: R) -> csv::Reader<R> {
    csv::ReaderBuilder::new().flexible(true).from_reader(input)
}

/// Redacts data rows `start..start + rows` of `input` into `output`, header first.
/// `progress` gets the rows and changed cells so far, every `PROGRESS_EVERY` rows and at the end.
pub fn redact_part<R: Read, W: Write>(
    input: R,
    output: W,
    policy: &Policy,
    start: u64,
    rows: u64,
    mut progress: impl FnMut(u64, u64),
) -> Result<Stats> {
    let mut input = reader(input);
    let mut writer = csv::WriterBuilder::new().flexible(true).from_writer(output);
    let header = input.headers()?.clone();
    let redactor = Redactor::new(policy, &header);
    writer.write_record(&header)?;
    let mut stats = Stats::default();
    let mut record = StringRecord::new();
    let mut seen = 0;
    while stats.rows < rows && input.read_record(&mut record)? {
        seen += 1;
        if seen <= start {
            continue;
        }
        let (out, changed) = redactor.row(&record);
        writer.write_record(&out)?;
        stats.rows += 1;
        stats.cells += changed;
        if stats.rows % PROGRESS_EVERY == 0 {
            progress(stats.rows, stats.cells);
        }
    }
    writer.flush()?;
    progress(stats.rows, stats.cells);
    Ok(stats)
}

/// The bytes `cells` take as a JSON array, escapes included.
fn json_len(cells: &[String]) -> usize {
    serde_json::to_vec(cells).map_or(usize::MAX, |v| v.len())
}

/// The first redacted rows that fit `max_rows` and `max_bytes`, and the file's total row count.
pub fn preview<R: Read>(
    input: R,
    policy: &Policy,
    max_rows: usize,
    max_bytes: usize,
) -> Result<Preview> {
    let mut input = reader(input);
    let header = input.headers()?.clone();
    let redactor = Redactor::new(policy, &header);
    let mut out = Preview {
        header: header.iter().map(str::to_owned).collect(),
        ..Preview::default()
    };
    let mut bytes = json_len(&out.header);
    let mut full = false;
    let mut record = StringRecord::new();
    while input.read_record(&mut record)? {
        out.total_rows += 1;
        if full || out.rows.len() >= max_rows {
            continue;
        }
        let (row, changed) = redactor.row(&record);
        let row: Vec<String> = row.iter().map(str::to_owned).collect();
        bytes += json_len(&row) + 1;
        if bytes > max_bytes {
            full = true;
            continue;
        }
        out.cells += changed;
        out.rows.push(row);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SALT: &str = "pepper";

    /// Hands out one byte per read, so every record spans read calls.
    struct OneByte<'a>(&'a [u8]);

    impl Read for OneByte<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.0.len().min(1).min(buf.len());
            buf[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Ok(n)
        }
    }

    fn policy(columns: &[(&str, Rule)]) -> Policy {
        Policy {
            salt: SALT.to_owned(),
            columns: columns.iter().map(|(c, r)| ((*c).to_owned(), *r)).collect(),
        }
    }

    fn run(input: &[u8], policy: &Policy, start: u64, rows: u64) -> (String, Stats) {
        let mut out = Vec::new();
        let stats = redact_part(input, &mut out, policy, start, rows, |_, _| {}).unwrap();
        (String::from_utf8(out).unwrap(), stats)
    }

    #[test]
    fn mask_keeps_a_hint() {
        assert_eq!(mask("jane.doe@acme.com"), "j***@***.com");
        assert_eq!(mask("4111-1111-1111-1234"), "****-****-****-**34");
        assert_eq!(mask("12345"), "***45");
        assert_eq!(mask("Jane"), "J***");
        assert_eq!(mask(""), "");
    }

    #[test]
    fn pseudonym_is_a_keyed_stable_token() {
        let token = pseudonym(SALT, "jane.doe@acme.com");
        assert_eq!(token, "P-b3f56ad63160");
        assert_eq!(token, pseudonym(SALT, "jane.doe@acme.com"));
        assert_ne!(token, pseudonym("other", "jane.doe@acme.com"));
        assert_ne!(token, pseudonym(SALT, "john.doe@acme.com"));
    }

    #[test]
    fn scan_replaces_pii_inside_text() {
        let text = "mail jane@acme.com or call +1 415 555 0132, (415) 555-0132, 415-555-0132 \
                    card 4111 1111 1111 1111 done";
        assert_eq!(
            scan(text),
            "mail [EMAIL] or call [PHONE], [PHONE], [PHONE] card [CARD] done"
        );
    }

    #[test]
    fn scan_leaves_other_text_and_digits_alone() {
        for text in [
            "nothing here, order 12345",
            "a@b",
            "a@b.c",
            "x@y.z.w",
            "12 345",
            "",
        ] {
            assert_eq!(scan(text), text);
        }
    }

    #[test]
    fn long_runs_do_not_slow_the_scan() {
        let digits = "7".repeat(200_000);
        let letters = "a".repeat(200_000);
        let start = std::time::Instant::now();
        scan(&digits);
        scan(&letters);
        assert!(start.elapsed().as_secs() < 5);
    }

    #[test]
    fn scan_keeps_non_ascii_text() {
        assert_eq!(
            scan("caf\u{e9} \u{4e2d}\u{6587} a@b.org \u{1f600}"),
            "caf\u{e9} \u{4e2d}\u{6587} [EMAIL] \u{1f600}"
        );
    }

    #[test]
    fn each_action_applies_to_its_column_and_unknown_columns_pass() {
        let p = policy(&[
            ("ssn", Rule::Drop),
            ("email", Rule::Mask),
            ("name", Rule::Pseudonym),
            ("note", Rule::Scan),
        ]);
        let input = "id,ssn,email,name,note,extra\n1,123-45-6789,jane@acme.com,Jane,ping jane@acme.com,keep\n";
        let (out, stats) = run(input.as_bytes(), &p, 0, 10);
        let want = format!(
            "id,ssn,email,name,note,extra\n1,,j***@***.com,{},ping [EMAIL],keep\n",
            pseudonym(SALT, "Jane")
        );
        assert_eq!(out, want);
        assert_eq!(stats, Stats { rows: 1, cells: 4 });
    }

    #[test]
    fn quoting_and_multiline_cells_survive() {
        let p = policy(&[("note", Rule::Scan)]);
        let input = "id,note\n1,\"line one\nline two, jane@acme.com\"\n2,\"say \"\"hi\"\" now\"\n";
        let (out, _) = run(input.as_bytes(), &p, 0, 10);
        assert_eq!(
            out,
            "id,note\n1,\"line one\nline two, [EMAIL]\"\n2,\"say \"\"hi\"\" now\"\n"
        );
    }

    #[test]
    fn reading_in_one_byte_pieces_changes_nothing() {
        let p = policy(&[("note", Rule::Scan), ("name", Rule::Pseudonym)]);
        let input = "id,name,note\n1,Jane,\"a\nb jane@acme.com\"\n2,Joe,call 415-555-0132\n";
        let whole = run(input.as_bytes(), &p, 0, 10).0;
        let mut out = Vec::new();
        redact_part(OneByte(input.as_bytes()), &mut out, &p, 0, 10, |_, _| {}).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), whole);
    }

    #[test]
    fn the_same_input_gives_the_same_bytes() {
        let p = policy(&[("name", Rule::Pseudonym), ("email", Rule::Mask)]);
        let input = "name,email\nJane,jane@acme.com\nJoe,joe@acme.com\n";
        assert_eq!(
            run(input.as_bytes(), &p, 0, 10),
            run(input.as_bytes(), &p, 0, 10)
        );
    }

    #[test]
    fn parts_join_into_the_whole_and_pseudonyms_match_across_them() {
        let p = policy(&[("name", Rule::Pseudonym), ("note", Rule::Scan)]);
        let mut input = String::from("name,note\n");
        for i in 0..7 {
            input += &format!("n{},mail a{i}@x.org\n", i % 3);
        }
        let (whole, _) = run(input.as_bytes(), &p, 0, 100);
        let mut joined = String::new();
        for (i, start) in [0, 3, 6].into_iter().enumerate() {
            let (part, _) = run(input.as_bytes(), &p, start, 3);
            assert!(part.starts_with("name,note\n"));
            joined += if i == 0 {
                &part
            } else {
                &part["name,note\n".len()..]
            };
        }
        assert_eq!(joined, whole);
    }

    #[test]
    fn progress_reports_rows_and_cells_done() {
        let p = policy(&[("a", Rule::Drop)]);
        let input = "a,b\n1,x\n2,y\n3,z\n";
        let mut seen = Vec::new();
        let mut out = Vec::new();
        redact_part(input.as_bytes(), &mut out, &p, 0, 10, |r, c| {
            seen.push((r, c))
        })
        .unwrap();
        assert_eq!(seen.last(), Some(&(3, 3)));
    }

    #[test]
    fn a_byte_order_mark_does_not_hide_the_first_column() {
        let p = policy(&[("ssn", Rule::Drop)]);
        let (out, _) = run("\u{feff}ssn,id\n123,1\n".as_bytes(), &p, 0, 10);
        assert_eq!(out, "ssn,id\n,1\n");
    }

    #[test]
    fn preview_stays_under_the_cap_when_json_escaping_inflates_cells() {
        let p = policy(&[]);
        let mut input = String::from("id,note\n");
        for i in 0..2000 {
            input += &format!("{i},\"{}\"\n", "\u{1}".repeat(300));
        }
        let pv = preview(input.as_bytes(), &p, 500, 48 * 1024).unwrap();
        assert!(!pv.rows.is_empty());
        assert!(serde_json::to_vec(&pv).unwrap().len() < 64 * 1024);
    }

    #[test]
    fn preview_is_capped_by_rows_and_bytes_and_counts_every_row() {
        let p = policy(&[("note", Rule::Scan)]);
        let mut input = String::from("id,note\n");
        for i in 0..2000 {
            input += &format!("{i},{}\n", "x".repeat(200));
        }
        let pv = preview(input.as_bytes(), &p, 500, 48 * 1024).unwrap();
        assert_eq!(pv.total_rows, 2000);
        assert!(pv.rows.len() < 500, "byte cap applies first");
        assert!(serde_json::to_vec(&pv).unwrap().len() < 64 * 1024);
        let small = preview(input.as_bytes(), &p, 5, 48 * 1024).unwrap();
        assert_eq!(small.rows.len(), 5);
        assert_eq!(small.header, ["id", "note"]);
    }
}
