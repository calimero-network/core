//! Every `meroctl` command the repository's Markdown shows parses against this
//! tree's command tree, so a renamed command or flag cannot stay in the docs.

use std::path::Path;
use std::process::Command;

use clap::error::ErrorKind;
use clap::Parser as _;

use super::{old_spellings, Cli};

const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const SEPARATORS: [&str; 9] = ["|", "||", "&&", ";", "&", "<", ">", ">>", "2>"]; // end a command
const MIN_COMMANDS: usize = 150; // fewer found means the extractor broke, not the docs

/// The `meroctl` commands in one shell line, each as its words; synopsis `[...]` groups dropped.
// shortcut: flags inside a synopsis' optional groups go unchecked; split them out if one drifts.
fn commands_in(line: &str) -> Result<Vec<Vec<String>>, ()> {
    let words = shlex::split(line).ok_or(())?;
    let mut found: Vec<Vec<String>> = Vec::new();
    let (mut at_start, mut open, mut substituted, mut optional) = (true, false, false, 0);
    for word in words {
        if SEPARATORS.contains(&word.as_str()) {
            (at_start, open) = (true, false);
            continue;
        }
        if open {
            let (word, closes) = match word.strip_suffix(')') {
                Some(rest) if substituted => (rest, true),
                _ => (word.as_str(), false),
            };
            let group = word.trim_end_matches("...");
            let opens = group.len() - group.trim_start_matches('[').len();
            let shuts = group.len() - group.trim_end_matches(']').len();
            if optional + opens == 0 && !word.is_empty() {
                found.last_mut().ok_or(())?.push(word.to_owned());
            }
            optional = (optional + opens).saturating_sub(shuts);
            open = !closes;
            continue;
        }
        let substitution = word.ends_with("$(meroctl");
        if substitution
            || (at_start
                && !word.contains('=')
                && (word == "meroctl" || word.ends_with("/meroctl")))
        {
            found.push(vec!["meroctl".to_owned()]);
            (open, substituted, optional) = (true, substitution, 0);
        }
        // A prompt or an environment assignment leaves the command still to come.
        at_start = at_start && (word == "$" || word.contains('='));
    }
    Ok(found)
}

/// Whether a line opens or closes a fenced code block.
fn is_fence(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("```") || line.starts_with("~~~")
}

/// What is wrong with the `meroctl` commands a Markdown file shows, and how many its code
/// blocks hold. Code blocks are parsed whole; an inline span is often only part of a
/// command, so it is held only to the spellings meroctl refuses.
fn problems(path: &str, text: &str) -> (Vec<String>, usize) {
    let (mut wrong, mut count) = (Vec::new(), 0);
    let (mut fenced, mut joined) = (false, String::new());
    for (n, line) in text.lines().enumerate() {
        let at = format!("{path}:{}", n + 1);
        if is_fence(line) {
            fenced = !fenced;
            joined.clear();
        } else if !fenced {
            for span in line.split('`').skip(1).step_by(2) {
                let words: Vec<String> = span.split_whitespace().map(str::to_owned).collect();
                if words.first().is_some_and(|w| w == "meroctl") {
                    for (old, new) in old_spellings(&words) {
                        wrong.push(format!("{at}: `{span}`: `{old}` was renamed `{new}`"));
                    }
                }
            }
        } else if let Some(head) = line.trim_end().strip_suffix('\\') {
            joined.push_str(head);
        } else {
            joined.push_str(line);
            let whole = std::mem::take(&mut joined);
            if !whole.contains("meroctl") {
                continue;
            }
            let Ok(commands) = commands_in(&whole) else {
                // Only a line that runs meroctl has to read as shell.
                if whole.trim_start().starts_with("meroctl ") {
                    wrong.push(format!("{at}: not a shell line: {}", whole.trim()));
                }
                continue;
            };
            for words in commands {
                count += 1;
                match Cli::try_parse_from(&words) {
                    Ok(_) => {}
                    Err(e)
                        if matches!(
                            e.kind(),
                            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
                        ) => {}
                    Err(e) => {
                        let why = e.to_string();
                        wrong.push(format!(
                            "{at}: {}\n    {}",
                            words.join(" "),
                            why.lines().next().unwrap_or_default()
                        ));
                    }
                }
            }
        }
    }
    (wrong, count)
}

#[test]
fn every_meroctl_command_in_the_docs_parses() {
    let listed = Command::new("git")
        .current_dir(REPO)
        .args(["ls-files", "-z", "--", "*.md", "*.mdx"])
        .output()
        .expect("git lists the repository's Markdown");
    assert!(listed.status.success(), "git ls-files failed");
    let (mut wrong, mut count) = (Vec::new(), 0);
    for path in String::from_utf8_lossy(&listed.stdout).split_terminator('\0') {
        // A changelog records what a release said at the time.
        if path.ends_with("CHANGELOG.md") {
            continue;
        }
        let text = std::fs::read_to_string(Path::new(REPO).join(path)).expect(path);
        let (found, n) = problems(path, &text);
        wrong.extend(found);
        count += n;
    }
    assert!(
        wrong.is_empty(),
        "the docs show meroctl commands this tree refuses:\n{}",
        wrong.join("\n")
    );
    assert!(count >= MIN_COMMANDS, "only {count} commands were found");
}

#[test]
fn commands_are_read_out_of_shell_lines() {
    let words = |line: &str| commands_in(line).unwrap();
    let one = |line: &str, want: &str| {
        assert_eq!(words(line), [want.split(' ').collect::<Vec<_>>()], "{line}");
    };
    one("meroctl whoami            # orgs: []", "meroctl whoami");
    one("$ meroctl whoami", "meroctl whoami");
    one("HOME=/tmp meroctl whoami && echo ok", "meroctl whoami");
    one("target/debug/meroctl whoami > out.json", "meroctl whoami");
    one("meroctl ops quotas < key", "meroctl ops quotas");
    one(
        "id=$(meroctl file upload a -i docs | jq -r .id)",
        "meroctl file upload a -i docs",
    );
    one("run=$(meroctl whoami)", "meroctl whoami");
    one(
        "meroctl call -i I get [--view [--min-index N]] [--args <json>]...",
        "meroctl call -i I get",
    );
    assert_eq!(words("meroctl whoami ; meroctl ops status").len(), 2);
    assert!(words("cargo build -p meroctl").is_empty());
    assert!(words("echo meroctl whoami").is_empty());
    assert!(words("MEROCTL_BINARY=target/profiling/meroctl scripts/soak.sh").is_empty());
    assert!(commands_in("meroctl call --args '{").is_err());
}

#[test]
fn a_refused_command_is_reported_where_it_stands() {
    let (wrong, count) = problems(
        "a.md",
        "Run `meroctl call -c kv get`.\n```bash\nmeroctl file upload ./a \\\n  -c docs\nmeroctl whoami\nmeroctl --help\nmeroctl call --args '{\n```\n",
    );
    assert_eq!(count, 3);
    assert_eq!(wrong.len(), 3, "{wrong:?}");
    assert!(wrong[0].starts_with("a.md:1: "), "{wrong:?}");
    assert!(wrong[1].starts_with("a.md:4: "), "{wrong:?}");
    assert!(wrong[1].contains("'-c'"), "{wrong:?}");
    assert!(
        wrong[2].starts_with("a.md:7: not a shell line"),
        "{wrong:?}"
    );
}
