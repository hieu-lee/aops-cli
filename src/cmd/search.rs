use crate::api::{self, Topic, SEARCH_PAGE};
use crate::args::{f, Cmd, Flag, Parsed};
use crate::net::Client;
use crate::out::*;
use crate::util::{clip, one_line, parse_period, tidy, Res};
use futures::future::try_join_all;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::fmt::Write;

const FILTER_FLAGS: &[Flag] = &[
    f!("since", "D", "created on/after date (YYYY|YYYY-MM|YYYY-MM-DD)"),
    f!("until", "D", "created before end of date"),
    f!("min-comments", "N", "comment_count >= N"),
    f!("max-comments", "N", "comment_count <= N"),
    f!("min-views", "N", "views >= N"),
    f!("sort", "S", "comments|views|new|old|rel (default rel/server order)"),
    f!(
        "pages",
        "N",
        "max result pages to fetch (default 8; 15 search / 10 tag results per page)"
    ),
    f!("full", "", "show full first post instead of a snippet"),
    f!("snip", "N", "snippet chars (default 160)"),
];

const SEARCH_FLAGS: &[Flag] = &[
    f!("title", "T", "title contains T"),
    f!("source", 'S', "S", "source field contains S (e.g. 'ISL 2024', 'China TST')"),
    f!("text", "T", "post text contains T"),
    f!("tags", "T", "tag text (as on AoPS)"),
    f!(
        "forum",
        "ID,..",
        "restrict to forum category ids (6=High School Olympiads, 5=Contests)"
    ),
    f!("any-post", "", "also match replies (default: first post only)"),
];

pub const SEARCH: Cmd = Cmd {
    name: "search",
    usage: "search [words] [--title --source --text --tags --forum] [filters]",
    about: "AoPS full-text search; pages fetched in parallel; client-side count/date/sort filters",
    flags: &[SEARCH_FLAGS, FILTER_FLAGS, COMMENT_FLAGS],
    examples: &[
        "search 'functional equation' --source ISL --min-comments 20 --sort comments -n 10",
        "search --tags 'Inversion' --since 2023 --forum 6 --full -n 5",
    ],
};

const TAG_FLAGS: &[Flag] = &[
    f!("forum", "ID", "forum category id (default 6 = High School Olympiads)"),
    f!("source", 'S', "RE", "source field matches regex (e.g. 'TST|ISL')"),
    f!("title", "RE", "title matches regex"),
];

pub const TAG: Cmd = Cmd {
    name: "tag",
    usage: "tag <tag> [more tags (AND)] [--forum ID] [--source RE] [filters]",
    about: "topics in a forum with a tag, newest activity first (first tag server-side, rest client-side)",
    flags: &[TAG_FLAGS, FILTER_FLAGS, COMMENT_FLAGS],
    examples: &[
        "tag inequalities --source 'ISL|IMO' --min-comments 10 -n 10",
        "tag 'Combinatorial Geometry' --forum 6 --since 2024 --full",
    ],
};

struct Flt {
    since: Option<i64>,
    until: Option<i64>,
    min_c: Option<i64>,
    max_c: Option<i64>,
    min_v: Option<i64>,
    sort: String,
    pages: usize,
}

impl Flt {
    fn new(p: &Parsed, default_pages: usize) -> Res<Flt> {
        let sort = p.get("sort").unwrap_or("rel").to_string();
        if !["comments", "views", "new", "old", "rel"].contains(&sort.as_str()) {
            return Err(format!("bad --sort '{sort}' (comments|views|new|old|rel)"));
        }
        Ok(Flt {
            since: p.get("since").map(|d| parse_period(d).map(|x| x.0)).transpose()?,
            until: p.get("until").map(|d| parse_period(d).map(|x| x.1)).transpose()?,
            min_c: p.num("min-comments")?,
            max_c: p.num("max-comments")?,
            min_v: p.num("min-views")?,
            sort,
            pages: p.num("pages")?.unwrap_or(default_pages).max(1),
        })
    }
    fn ok(&self, t: &Topic) -> bool {
        self.since.is_none_or(|s| t.created >= s)
            && self.until.is_none_or(|u| t.created < u)
            && self.min_c.is_none_or(|v| t.comments >= v)
            && self.max_c.is_none_or(|v| t.comments <= v)
            && self.min_v.is_none_or(|v| t.views >= v)
    }
    fn sort(&self, v: &mut [Topic]) {
        match self.sort.as_str() {
            "comments" => v.sort_by_key(|t| std::cmp::Reverse(t.comments)),
            "views" => v.sort_by_key(|t| std::cmp::Reverse(t.views)),
            "new" => v.sort_by_key(|t| std::cmp::Reverse(t.created)),
            "old" => v.sort_by_key(|t| t.created),
            _ => {}
        }
    }
}

fn settings(p: &Parsed) -> Res<String> {
    let mut m = Map::new();
    let words = p.pos.join(" ");
    let expanded = ["title", "source", "text", "tags"].iter().any(|k| p.get(k).is_some());
    if !words.is_empty() {
        m.insert(if expanded { "post_text" } else { "search_text" }.into(), json!(words));
    }
    for (flag, key) in [("title", "title"), ("source", "source"), ("tags", "tags")] {
        if let Some(v) = p.get(flag) {
            m.insert(key.into(), json!(v));
        }
    }
    if let Some(v) = p.get("text") {
        let prev = m
            .get("post_text")
            .and_then(Value::as_str)
            .map(|s| format!("{s} {v}"))
            .unwrap_or(v.to_string());
        m.insert("post_text".into(), json!(prev));
    }
    if m.is_empty() {
        return Err("search: give words or --title/--source/--text/--tags".into());
    }
    m.insert("first_post".into(), json!(if p.flag("any-post") { 0 } else { 1 }));
    if let Some(v) = p.get("forum") {
        let ids: Vec<i64> = crate::util::split_list(v)
            .iter()
            .map(|x| x.parse().map_err(|_| format!("--forum: bad id '{x}'")))
            .collect::<Res<_>>()?;
        m.insert("forums".into(), json!(ids));
        m.insert("forums_action".into(), json!("include"));
    }
    if let Some(d) = p.get("since") {
        m.insert("start_date".into(), json!(parse_period(d)?.0));
    }
    if let Some(d) = p.get("until") {
        m.insert("end_date".into(), json!(parse_period(d)?.1));
    }
    Ok(Value::Object(m).to_string())
}

pub async fn search(c: &Client, p: Parsed, _stdin: Option<String>) -> Res<String> {
    let o = Out::new(&p, 15)?;
    let fl = Flt::new(&p, 8)?;
    let co = CommentOpts::new(&p)?;
    let st = settings(&p)?;
    let want = o.offset + o.limit;
    let sorted = !matches!(fl.sort.as_str(), "rel");
    let mut got: Vec<Topic> = vec![];
    let mut seen = HashSet::new();
    let mut page = 0;
    let mut more = true;
    let wave = 4;
    while more && page < fl.pages && (sorted || o.fmt == Fmt::Count || got.len() < want) {
        let n = wave.min(fl.pages - page);
        let pages = try_join_all((page..page + n).map(|i| api::search(c, &st, i * SEARCH_PAGE))).await?;
        for pg in pages {
            more &= pg.more;
            for t in &pg.topics {
                if seen.insert(t.id) && fl.ok(t) {
                    got.push(t.clone());
                }
            }
        }
        page += n;
    }
    fl.sort(&mut got);
    emit(c, got, &p, &o, &co, if more && page >= fl.pages { Some(page) } else { None }).await
}

pub async fn tag(c: &Client, p: Parsed, _stdin: Option<String>) -> Res<String> {
    let o = Out::new(&p, 10)?;
    let fl = Flt::new(&p, 8)?;
    let co = CommentOpts::new(&p)?;
    if p.pos.is_empty() {
        return Err("tag: give a tag (e.g. inequalities)".into());
    }
    let forum: i64 = p.num("forum")?.unwrap_or(6);
    let src = p.get("source").map(regex).transpose()?;
    let title = p.get("title").map(regex).transpose()?;
    let tag_id = api::tag_id(c, &p.pos[0]).await?;
    let extra: Vec<String> = p.pos[1..].iter().map(|t| t.to_lowercase()).collect();
    let want = o.offset + o.limit;
    let sorted = !matches!(fl.sort.as_str(), "rel");
    let mut got: Vec<Topic> = vec![];
    let mut seen = HashSet::new();
    let mut before = 99_999_999_999i64;
    let mut page = 0;
    let mut more = true;
    while more && page < fl.pages && (sorted || o.fmt == Fmt::Count || got.len() < want) {
        let pg = api::forum_tag(c, forum, tag_id, before).await?;
        page += 1;
        let min_last = pg.topics.iter().map(|t| t.last).filter(|&l| l > 0).min();
        for t in &pg.topics {
            let tags: Vec<String> = t.tags.iter().map(|x| x.to_lowercase()).collect();
            let ok = fl.ok(t)
                && extra.iter().all(|e| tags.contains(e))
                && src.as_ref().is_none_or(|r| r.is_match(&t.source))
                && title.as_ref().is_none_or(|r| r.is_match(&t.title));
            if seen.insert(t.id) && ok {
                got.push(t.clone());
            }
        }
        more = pg.more;
        match min_last {
            Some(m) if m < before => before = m,
            _ => more = false,
        }
        if fl.since.is_some_and(|s| before < s) {
            more = false;
        }
    }
    fl.sort(&mut got);
    emit(c, got, &p, &o, &co, if more && page >= fl.pages { Some(page) } else { None }).await
}

async fn emit(c: &Client, got: Vec<Topic>, p: &Parsed, o: &Out, co: &CommentOpts, capped: Option<usize>) -> Res<String> {
    let total = got.len();
    if let (Some(n), true) = (capped, o.fmt != Fmt::Txt) {
        crate::util::warn(&format!("stopped after {n} pages; results incomplete (--pages N)"));
    }
    if o.fmt == Fmt::Count {
        return Ok(format!("{total}\n"));
    }
    let mut page: Vec<std::sync::Arc<Topic>> = got.into_iter().skip(o.offset).take(o.limit).map(std::sync::Arc::new).collect();
    if co.wants() {
        let ts = futures::future::join_all(page.iter().map(|t| api::topic_all(c, t.id))).await;
        for (slot, r) in page.iter_mut().zip(ts) {
            match r {
                Ok(t) => *slot = t,
                Err(e) => crate::util::fail(&format!("comments for topic {} unavailable: {e}", slot.id)),
            }
        }
    }
    let full = p.flag("full");
    let snip: usize = p.num("snip")?.unwrap_or(160);
    let mut s = String::new();
    for t in &page {
        let body = if full {
            tidy(t.statement())
        } else {
            clip(&one_line(t.statement()), snip)
        };
        match o.fmt {
            Fmt::Ids => {
                let _ = writeln!(s, "{}", t.id);
            }
            Fmt::Json => {
                let mut v = json!({"id": t.id, "title": t.title, "source": t.source, "forum": t.forum, "tags": t.tags,
                    "comments_count": t.comments, "views": t.views, "created": t.created, "text": clip(&body, if full { o.trunc } else { 0 })});
                if co.wants() {
                    v["comments"] = json!(co.select(t).iter().map(|x| co.json(x)).collect::<Vec<_>>());
                }
                let _ = writeln!(s, "{v}");
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
            }
            _ => {
                let mut head = format!("#{} {}", t.id, t.title);
                if !t.source.is_empty() && t.source != t.title {
                    let _ = write!(head, " | {}", t.source);
                }
                let _ = write!(head, " | {} | {} | {}", t.forum, date(t.created), counters(t));
                if !t.tags.is_empty() {
                    let _ = write!(head, " | {}", tag_list(&t.tags));
                }
                let _ = writeln!(s, "{head}");
                if !body.is_empty() {
                    let _ = writeln!(s, "{}", if full { clip(&body, o.trunc) } else { body });
                }
                if co.wants() {
                    write_comments(&mut s, co, t);
                }
            }
        }
    }
    if matches!(o.fmt, Fmt::Txt) {
        let _ = write!(s, "-- {} of {total} results", page.len());
        if o.offset + page.len() < total {
            let _ = write!(s, "; more: --offset {}", o.offset + page.len());
        }
        if let Some(n) = capped {
            let _ = write!(s, "; stopped at {n} pages (--pages for more)");
        }
        s.push('\n');
    }
    Ok(s)
}
