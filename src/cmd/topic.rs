use super::browse::stdin_items;
use crate::api::{self, Topic};
use crate::args::{f, Cmd, Flag, Parsed};
use crate::net::Client;
use crate::out::*;
use crate::util::{clip, tidy, unesc, Res};
use futures::future::join_all;
use serde_json::json;
use std::fmt::Write;
use std::sync::Arc;

const FLAGS: &[Flag] = &[
    f!("no-text", "", "omit the statement (comments only)"),
    f!("meta-only", "", "only title/counters/tags"),
];

pub const TOPIC: Cmd = Cmd {
    name: "topic",
    usage: "topic <id|url ...> [-c N] [comment filters]",
    about: "fetch topics: statement + top comments (default -c 0). Parallel for many ids",
    flags: &[FLAGS, COMMENT_FLAGS],
    examples: &[
        "topic 3866863 -c 5 --sol",
        "topic 3358923 3358932 -c 3 --csort thanks --cmin-len 200 --ctrunc 1200",
    ],
};

/// "3866863", "h3866863", or an AoPS community URL -> topic id.
pub fn parse_id(s: &str) -> Option<i64> {
    let s = s.trim().trim_start_matches('#');
    if let Ok(n) = s.parse() {
        return Some(n);
    }
    let b = s.as_bytes();
    let i = (0..b.len().saturating_sub(1)).find(|&i| b[i] == b'h' && b[i + 1].is_ascii_digit())?;
    let digits: String = s[i + 1..].chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

pub async fn run(c: &Client, p: Parsed, stdin: Option<String>) -> Res<String> {
    let o = Out::new(&p, 50)?;
    let co = CommentOpts::new(&p)?;
    let mut ids = vec![];
    for w in &p.pos {
        for x in if w == "-" {
            stdin_items(stdin.as_deref())?
        } else {
            vec![w.clone()]
        } {
            ids.push(parse_id(&x).ok_or_else(|| format!("topic: bad id/url '{x}'"))?);
        }
    }
    if ids.is_empty() {
        return Err("topic: give topic id(s) or URL(s)".into());
    }
    let mut seen = std::collections::HashSet::new();
    ids.retain(|i| seen.insert(*i));
    let total = ids.len();
    if o.fmt == Fmt::Count {
        return Ok(format!("{total}\n"));
    }
    let ids: Vec<i64> = ids.into_iter().skip(o.offset).take(o.limit).collect();
    let need_posts = co.wants() && !p.flag("meta-only");
    let res = join_all(ids.iter().map(|&id| async move {
        if need_posts {
            api::topic_all(c, id).await
        } else {
            api::topic(c, id).await
        }
    }))
    .await;
    let mut s = String::new();
    for (id, r) in ids.iter().zip(res) {
        match r {
            Err(e) => {
                crate::util::fail(&format!("topic {id}: {e}"));
                if o.fmt == Fmt::Txt {
                    let _ = writeln!(s, "#{id} !! {e}");
                }
            }
            Ok(t) => render(&mut s, &t, &p, &co, &o),
        }
    }
    Ok(s)
}

fn render(s: &mut String, t: &Arc<Topic>, p: &Parsed, co: &CommentOpts, o: &Out) {
    match o.fmt {
        Fmt::Ids => {
            let _ = writeln!(s, "{}", t.id);
            return;
        }
        Fmt::Json => {
            let mut v = json!({"id": t.id, "title": t.title, "source": t.source, "forum": unesc(&t.forum), "tags": t.tags,
                "comments_count": t.comments, "views": t.views, "created": t.created});
            if !p.flag("no-text") && !p.flag("meta-only") {
                v["text"] = json!(clip(&tidy(t.statement()), o.trunc));
            }
            if co.wants() && !p.flag("meta-only") {
                v["comments"] = json!(co.select(t).iter().map(|x| co.json(x)).collect::<Vec<_>>());
            }
            let _ = writeln!(s, "{v}");
            return;
        }
        Fmt::Tsv => {
            let _ = writeln!(
                s,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                t.id,
                date(t.created),
                t.comments,
                t.views,
                t.source,
                t.title,
                tag_list(&t.tags)
            );
            return;
        }
        _ => {}
    }
    let mut head = format!("#{} {}", t.id, t.title);
    if !t.source.is_empty() && t.source != t.title {
        let _ = write!(head, " | {}", t.source);
    }
    let _ = write!(head, " | {} | {} | {}", t.forum, date(t.created), counters(t));
    if !t.tags.is_empty() {
        let _ = write!(head, " | {}", tag_list(&t.tags));
    }
    let _ = writeln!(s, "{head}");
    if p.flag("meta-only") {
        return;
    }
    if !p.flag("no-text") {
        let _ = writeln!(s, "{}", clip(&tidy(t.statement()), o.trunc));
    }
    if co.wants() {
        write_comments(s, co, t);
    }
}

#[cfg(test)]
mod tests {
    use super::parse_id;
    #[test]
    fn ids() {
        assert_eq!(parse_id("3866863"), Some(3866863));
        assert_eq!(
            parse_id("https://artofproblemsolving.com/community/c6h3866863p38676959"),
            Some(3866863)
        );
        assert_eq!(parse_id("h12"), Some(12));
        assert_eq!(parse_id("#77"), Some(77));
        assert_eq!(parse_id("abc"), None);
    }
}
