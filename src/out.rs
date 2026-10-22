use crate::api::{Post, Topic};
use crate::args::{f, Flag, Parsed};
use crate::util::{clip, tidy, ymd, Res};
use regex::{Regex, RegexBuilder};
use std::fmt::Write;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fmt {
    Txt,
    Json,
    Ids,
    Tsv,
    Count,
}

pub struct Out {
    pub fmt: Fmt,
    pub trunc: usize,
    pub limit: usize,
    pub offset: usize,
}

impl Out {
    pub fn new(p: &Parsed, default_limit: usize) -> Res<Out> {
        let fmt = match p.get("fmt").unwrap_or("txt") {
            "txt" | "text" => Fmt::Txt,
            "json" | "jsonl" | "ndjson" => Fmt::Json,
            "ids" | "id" => Fmt::Ids,
            "tsv" => Fmt::Tsv,
            "count" => Fmt::Count,
            o => return Err(format!("bad --fmt '{o}' (txt|json|ids|tsv|count)")),
        };
        Ok(Out {
            fmt,
            trunc: p.num("trunc")?.unwrap_or(1500),
            limit: p.limit(default_limit)?,
            offset: p.num("offset")?.unwrap_or(0),
        })
    }
}

pub fn regex(pat: &str) -> Res<Regex> {
    RegexBuilder::new(pat)
        .case_insensitive(true)
        .build()
        .map_err(|e| format!("bad regex '{pat}': {e}"))
}

/// Case-insensitive keyword filter: all `-k` flags must match; commas inside one flag are alternatives.
pub struct Keywords(Vec<Vec<String>>);

impl Keywords {
    pub fn new(vals: &[&str]) -> Keywords {
        Keywords(
            vals.iter()
                .map(|v| v.split(',').map(|a| a.trim().to_lowercase()).filter(|a| !a.is_empty()).collect())
                .collect(),
        )
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn matches(&self, hay: &str) -> bool {
        if self.0.is_empty() {
            return true;
        }
        let h = hay.to_lowercase();
        self.0.iter().all(|alts| alts.iter().any(|a| h.contains(a.as_str())))
    }
}

pub const COMMENT_FLAGS: &[Flag] = &[
    f!("comments", 'c', "N", "attach top N comments per topic (default 0)"),
    f!("csort", "S", "comment order: thanks|time|len|last (default thanks)"),
    f!("cmin-thanks", "N", "only comments with >=N thanks"),
    f!("cmin-len", "N", "only comments with >=N chars"),
    f!("cgrep", "RE", "only comments matching regex"),
    f!("cuser", "U,..", "only comments by these users"),
    f!("ctrunc", "N", "clip each comment to N chars, 0=none (default 600)"),
    f!("sol", "", "only solution-like comments (long, [hide], \\boxed, 'solution')"),
];

pub struct CommentOpts {
    pub n: usize,
    sort: String,
    min_thanks: i64,
    min_len: usize,
    grep: Option<Regex>,
    users: Vec<String>,
    trunc: usize,
    sol: bool,
}

impl CommentOpts {
    pub fn new(p: &Parsed) -> Res<CommentOpts> {
        let sort = p.get("csort").unwrap_or("thanks").to_string();
        if !["thanks", "time", "len", "last"].contains(&sort.as_str()) {
            return Err(format!("bad --csort '{sort}' (thanks|time|len|last)"));
        }
        Ok(CommentOpts {
            // id/tsv/count listings never show comments, so never fetch them
            n: if matches!(p.get("fmt"), Some("ids" | "id" | "tsv" | "count")) {
                0
            } else {
                p.num("comments")?.unwrap_or(0)
            },
            sort,
            min_thanks: p.num("cmin-thanks")?.unwrap_or(0),
            min_len: p.num("cmin-len")?.unwrap_or(0),
            grep: p.get("cgrep").map(regex).transpose()?,
            users: p
                .get("cuser")
                .map(|u| crate::util::split_list(&u.to_lowercase()))
                .unwrap_or_default(),
            trunc: p.num("ctrunc")?.unwrap_or(600),
            sol: p.flag("sol"),
        })
    }

    pub fn wants(&self) -> bool {
        self.n > 0
    }

    pub fn select<'a>(&self, t: &'a Topic) -> Vec<&'a Post> {
        let mut v: Vec<&Post> = t
            .posts
            .iter()
            .filter(|p| p.n != 1)
            .filter(|p| p.thanks >= self.min_thanks)
            .filter(|p| p.text.chars().count() >= self.min_len)
            .filter(|p| self.grep.as_ref().is_none_or(|r| r.is_match(&p.text)))
            .filter(|p| self.users.is_empty() || self.users.contains(&p.user.to_lowercase()))
            .filter(|p| !self.sol || solution_like(&p.text))
            .collect();
        match self.sort.as_str() {
            "time" => v.sort_by_key(|p| p.n),
            "last" => v.sort_by_key(|p| std::cmp::Reverse(p.n)),
            "len" => v.sort_by_key(|p| std::cmp::Reverse(p.text.len())),
            _ => v.sort_by_key(|p| (std::cmp::Reverse(p.thanks), p.n)),
        }
        v.truncate(self.n);
        v
    }

    pub fn render(&self, p: &Post) -> String {
        let th = if p.thanks > 0 { format!(" +{}", p.thanks) } else { String::new() };
        format!("[#{} @{}{th}] {}", p.n, p.user, clip(&tidy(&p.text), self.trunc))
    }

    pub fn json(&self, p: &Post) -> serde_json::Value {
        serde_json::json!({"n": p.n, "user": p.user, "thanks": p.thanks, "time": p.time, "text": clip(&tidy(&p.text), self.trunc)})
    }
}

fn solution_like(t: &str) -> bool {
    t.chars().count() >= 400 || t.contains("[hide") || t.contains("\\boxed") || t.to_lowercase().contains("solution")
}

pub fn tag_list(tags: &[String]) -> String {
    tags.iter().take(8).cloned().collect::<Vec<_>>().join(",")
}

/// `c32 v2861` style counters; blank when the topic data is unknown.
pub fn counters(t: &Topic) -> String {
    format!("c{} v{}", t.comments, t.views)
}

pub fn write_comments(out: &mut String, co: &CommentOpts, t: &Topic) {
    for p in co.select(t) {
        let _ = writeln!(out, "{}", co.render(p));
    }
}

pub fn date(ts: i64) -> String {
    if ts > 0 {
        ymd(ts)
    } else {
        "?".into()
    }
}
