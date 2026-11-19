//! lineworker keeps no database. What it did is written as one hidden line at the start of
//! its comments on a pull request and read back from there:
//!
//! - `<!-- lineworker: notified -->`: the author's session was told;
//! - `<!-- lineworker: rerun head=SHA -->`: an infrastructure failure of SHA was rerun;
//! - `<!-- lineworker: repair from=SHA pushed=SHA -->` (`pushed=none` when nothing was
//!   pushed): a repair ran.

use crate::model::{History, Repair, Time};

/// The marker line of a comment, for lineworker to write.
pub fn marker(kind: &str, fields: &[(&str, &str)]) -> String {
    let mut out = format!("<!-- lineworker: {kind}");
    for (k, v) in fields {
        out.push_str(&format!(" {k}={v}"));
    }
    out.push_str(" -->");
    out
}

/// Reads the history of a pull request from its comments: `(body, created_at)`.
pub fn history<'a>(comments: impl IntoIterator<Item = (&'a str, Time)>) -> History {
    let mut h = History::default();
    for (body, at) in comments {
        let Some((kind, fields)) = parse(body) else {
            continue;
        };
        let field = |name: &str| {
            fields
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        };
        match kind {
            "notified" => h.notified_author_at = h.notified_author_at.max(Some(at)),
            "rerun" => {
                if let Some(head) = field("head") {
                    h.reruns.push(head);
                }
            }
            "repair" => {
                if let Some(from_head) = field("from") {
                    h.repairs.push(Repair {
                        at,
                        from_head,
                        pushed_head: field("pushed").filter(|p| p != "none"),
                    });
                }
            }
            _ => {}
        }
    }
    h
}

/// Whether a comment was written by lineworker.
pub fn is_marked(body: &str) -> bool {
    parse(body).is_some()
}

fn parse(body: &str) -> Option<(&str, Vec<(&str, &str)>)> {
    let line = body.trim_start().lines().next()?.trim();
    let inner = line.strip_prefix("<!-- lineworker:")?.strip_suffix("-->")?;
    let mut words = inner.split_whitespace();
    let kind = words.next()?;
    let fields = words.filter_map(|w| w.split_once('=')).collect();
    Some((kind, fields))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_are_written_and_read_back() {
        let notified = marker("notified", &[]);
        let rerun = marker("rerun", &[("head", "abc")]);
        let failed = format!(
            "{}\nThe check still fails.",
            marker("repair", &[("from", "abc"), ("pushed", "none")])
        );
        let fixed = marker("repair", &[("from", "def"), ("pushed", "fed")]);
        assert_eq!(notified, "<!-- lineworker: notified -->");
        let h = history([
            (notified.as_str(), 10),
            ("plain comment <!-- lineworker: notified -->", 99),
            (rerun.as_str(), 20),
            (failed.as_str(), 30),
            (fixed.as_str(), 40),
            ("<!-- lineworker: notified -->", 50),
        ]);
        assert_eq!(h.notified_author_at, Some(50));
        assert_eq!(h.reruns, vec!["abc".to_string()]);
        assert_eq!(h.repairs.len(), 2);
        assert_eq!(h.repairs[0].pushed_head, None);
        assert_eq!(h.repairs[1].pushed_head.as_deref(), Some("fed"));
        assert!(is_marked(&fixed) && !is_marked("plain comment"));
    }
}
