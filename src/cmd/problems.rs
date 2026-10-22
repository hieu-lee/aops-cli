use super::browse::{query_from, years_from, SELECT_FLAGS, YEARS_FLAG};
use crate::api::{self, Topic};
use crate::args::{f, Cmd, Flag, Parsed};
use crate::index::{self, Exam};
use crate::net::Client;
use crate::out::*;
use crate::util::{clip, one_line, tidy, Res};
use futures::future::join_all;
use serde_json::json;
use std::fmt::Write;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Prob {
    pub topic: i64,
    pub exam: Arc<Exam>,
    pub heading: String,
    pub label: String,
    pub pos: usize,
    pub of: usize,
    pub last_in_group: bool,
    pub text: String,
}

/// Problems of one exam category, in listed order.
pub async fn exam_problems(c: &Client, e: &Arc<Exam>) -> Res<Vec<Prob>> {
    let cat = api::category(c, e.id).await?;
    Ok(parse_exam(&cat, e))
}

fn parse_exam(cat: &api::Cat, e: &Arc<Exam>) -> Vec<Prob> {
    let mut v: Vec<Prob> = vec![];
    let mut heading = String::new();
    let mut group = 0usize;
    let mut groups: Vec<usize> = vec![];
    for it in &cat.items {
        let Some(pd) = &it.post_data else { continue };
        if it.item_type == "post_hidden" {
            continue;
        }
        if pd.topic_id == 0 || pd.post_type == "view_posts_text" {
            let h = one_line(&pd.post_canonical);
            if !h.is_empty() {
                heading = clip(&h, 60);
                group += 1;
            }
            continue;
        }
        groups.push(group);
        v.push(Prob {
            topic: pd.topic_id,
            exam: e.clone(),
            heading: heading.clone(),
            label: one_line(&it.item_text),
            pos: v.len() + 1,
            of: 0,
            last_in_group: false,
            text: tidy(&pd.post_canonical),
        });
    }
    let n = v.len();
    for i in 0..n {
        v[i].of = n;
        v[i].last_in_group = i + 1 == n || groups[i + 1] != groups[i];
    }
    v
}

pub const FLAGS: &[Flag] = &[
    f!("kw", 'k', "W,..", "statement/title contains any of W (repeat flag = AND)"),
    f!("label", "RE", "problem label regex (e.g. '^[AG][5-8]$', '[36]$')"),
    f!("pos", "N,..", "ordinal in exam; negative counts from end (3,6 | -1)"),
    f!("hard", "", "only last problem of each heading group (IMO P3/P6, ISL A7/C7/G8/N8)"),
    f!("min-comments", "N", "needs >=N comments (AoPS comment_count)"),
    f!("max-comments", "N", "needs <=N comments"),
    f!("min-views", "N", "needs >=N views"),
    f!("tag", "T,..", "topic has any tag T (repeat flag = AND)"),
    f!("sort", "S", "year (default)|comments|views|random"),
    f!("rev", "", "reverse the final order"),
    f!("meta", "", "always show comments/views/tags (fetches topics)"),
    f!("no-text", "", "omit statements"),
    f!("max-exams", "N", "fetch at most N newest exams (default 120)"),
    f!("scan", "N", "max topics fetched for metadata filters (default 300)"),
];

pub const PROBLEMS: Cmd = Cmd {
    name: "problems",
    usage: "problems <contest words|id ...> [--country --kind --years] [filters] [-c N]",
    about: "fetch exam problems (statement in LaTeX) with filters; -c attaches top comments",
    flags: &[SELECT_FLAGS, YEARS_FLAG, FLAGS, COMMENT_FLAGS],
    examples: &[
        "problems imo --years 2023-2025 --hard",
        "problems --kind tst -C china,iran -y 2023- --min-comments 15 --sort comments -n 10 --no-text",
        "problems 'isl|shortlist' -y 2022 -k 'functional equation' -c 3",
    ],
};

struct Filters {
    kw: Keywords,
    label: Option<regex::Regex>,
    pos: Vec<i64>,
    hard: bool,
    min_c: Option<i64>,
    max_c: Option<i64>,
    min_v: Option<i64>,
    tags: Vec<Vec<String>>,
}

impl Filters {
    fn new(p: &Parsed) -> Res<Filters> {
        let mut pos = vec![];
        for v in p.all("pos") {
            for x in crate::util::split_list(v) {
                pos.push(x.parse::<i64>().map_err(|_| format!("--pos: bad number '{x}'"))?);
            }
        }
        Ok(Filters {
            kw: Keywords::new(&p.all("kw")),
            label: p.get("label").map(regex).transpose()?,
            pos,
            hard: p.flag("hard"),
            min_c: p.num("min-comments")?,
            max_c: p.num("max-comments")?,
            min_v: p.num("min-views")?,
            tags: p.all("tag").iter().map(|t| crate::util::split_list(&t.to_lowercase())).collect(),
        })
    }

    fn needs_topic(&self) -> bool {
        self.min_c.is_some() || self.max_c.is_some() || self.min_v.is_some() || !self.tags.is_empty()
    }

    fn cheap(&self, p: &Prob) -> bool {
        if self.hard && !p.last_in_group {
            return false;
        }
        if !self.pos.is_empty()
            && !self
                .pos
                .iter()
                .any(|&v| v == p.pos as i64 || (v < 0 && p.pos as i64 == p.of as i64 + v + 1))
        {
            return false;
        }
        if self.label.as_ref().is_some_and(|r| !r.is_match(&p.label)) {
            return false;
        }
        self.kw.is_empty() || self.kw.matches(&format!("{} {} {}", p.text, p.exam.name, p.heading))
    }

    fn topic_ok(&self, t: &Topic) -> bool {
        self.min_c.is_none_or(|v| t.comments >= v)
            && self.max_c.is_none_or(|v| t.comments <= v)
            && self.min_v.is_none_or(|v| t.views >= v)
            && self.tags.iter().all(|alts| t.tags.iter().any(|g| alts.contains(&g.to_lowercase())))
    }
}

struct Row {
    p: Prob,
    t: Option<Arc<Topic>>,
}

/// Run the command; any partial fetch failure marks the process failed and is reported on stderr for machine formats.
pub async fn run(c: &Client, p: Parsed, stdin: Option<String>) -> Res<String> {
    let fmt = Out::new(&p, 20)?.fmt;
    let mut warns = vec![];
    let r = run_inner(c, p, stdin, &mut warns).await;
    if !warns.is_empty() {
        if fmt != Fmt::Txt {
            for w in &warns {
                crate::util::warn(w);
            }
        }
        crate::util::mark_failed();
    }
    r
}

async fn run_inner(c: &Client, p: Parsed, stdin: Option<String>, warns: &mut Vec<String>) -> Res<String> {
    let o = Out::new(&p, 20)?;
    let q = query_from(&p, stdin.as_deref())?;
    if q.words.is_empty() && q.ids.is_empty() && q.countries.is_empty() && q.kinds.is_empty() {
        return Err("problems: give contest words/ids or --country/--kind (try: aops find, aops help problems)".into());
    }
    let fl = Filters::new(&p)?;
    let co = CommentOpts::new(&p)?;
    let sort = p.get("sort").unwrap_or("year").to_string();
    if !["year", "comments", "views", "random"].contains(&sort.as_str()) {
        return Err(format!("bad --sort '{sort}' (year|comments|views|random)"));
    }
    let scan: usize = p.num("scan")?.unwrap_or(300);
    let meta = p.flag("meta") || co.wants();

    let contests = index::find(c, &q).await?;
    if contests.is_empty() {
        return Err("no contest matches (see: aops find <words>)".into());
    }
    let mut exams: Vec<Arc<Exam>> = index::exams(c, &contests, &years_from(&p)?)
        .await?
        .into_iter()
        .map(Arc::new)
        .collect();
    let max_exams: usize = p.num("max-exams")?.unwrap_or(120);
    let exams_total = exams.len();
    exams.truncate(max_exams);
    let lists = join_all(exams.iter().map(|e| exam_problems(c, e))).await;
    let mut cands: Vec<Prob> = vec![];
    for (e, r) in exams.iter().zip(lists) {
        match r {
            Ok(v) => cands.extend(v.into_iter().filter(|x| fl.cheap(x))),
            Err(err) => warns.push(format!("exam {} ({}): {err}", e.id, e.name)),
        }
    }
    let n_exams = exams.len();

    if sort == "random" {
        shuffle(&mut cands);
    }
    let sort_meta = matches!(sort.as_str(), "comments" | "views");
    let need_all = sort_meta || p.flag("rev");
    let need_meta = fl.needs_topic() || sort_meta;
    let want = o.offset + o.limit;
    let mut rows: Vec<Row> = vec![];
    let mut scanned = 0usize;
    let mut partial = false;

    if need_meta {
        let mut idx = 0;
        while idx < cands.len() && scanned < scan && (need_all || o.fmt == Fmt::Count || rows.len() < want) {
            let wave = if need_all || o.fmt == Fmt::Count {
                scan
            } else {
                (2 * (want - rows.len())).max(16)
            };
            let end = (idx + wave).min(cands.len()).min(idx + (scan - scanned));
            let chunk: Vec<Prob> = cands[idx..end].to_vec();
            scanned += chunk.len();
            let ts = join_all(chunk.iter().map(|x| api::topic(c, x.topic))).await;
            for (x, t) in chunk.into_iter().zip(ts) {
                match t {
                    Ok(t) if fl.topic_ok(&t) => rows.push(Row { p: x, t: Some(t) }),
                    Ok(_) => {}
                    Err(e) => warns.push(format!("topic {}: {e}", x.topic)),
                }
            }
            idx = end;
        }
        partial = idx < cands.len();
        scanned = idx;
    } else {
        rows = cands.into_iter().map(|p| Row { p, t: None }).collect();
    }

    match sort.as_str() {
        "comments" => rows.sort_by_key(|r| std::cmp::Reverse(r.t.as_ref().map_or(0, |t| t.comments))),
        "views" => rows.sort_by_key(|r| std::cmp::Reverse(r.t.as_ref().map_or(0, |t| t.views))),
        _ => {}
    }
    if p.flag("rev") {
        rows.reverse();
    }
    let total = rows.len();
    if o.fmt != Fmt::Txt {
        if partial {
            crate::util::warn(&format!("metadata scan stopped at {scanned} topics; results incomplete (--scan N)"));
        }
        if exams_total > exams.len() {
            crate::util::warn(&format!(
                "only the newest {} of {exams_total} exams were read (--max-exams)",
                exams.len()
            ));
        }
    }
    if o.fmt == Fmt::Count {
        return Ok(format!("{total}\n"));
    }
    let mut page: Vec<Row> = rows.into_iter().skip(o.offset).take(o.limit).collect();

    // metadata / comments for shown rows only
    if meta || o.fmt == Fmt::Json {
        let need: Vec<usize> = (0..page.len()).filter(|&i| page[i].t.is_none() || co.wants()).collect();
        let ids: Vec<i64> = need.iter().map(|&i| page[i].p.topic).collect();
        let all = co.wants();
        let ts = join_all(ids.iter().map(|&id| async move {
            if all {
                api::topic_all(c, id).await
            } else {
                api::topic(c, id).await
            }
        }))
        .await;
        for (i, t) in need.into_iter().zip(ts) {
            match t {
                Ok(t) => page[i].t = Some(t),
                Err(e) => warns.push(format!("topic {}: {e}", page[i].p.topic)),
            }
        }
    }

    let mut s = String::new();
    let no_text = p.flag("no-text");
    match o.fmt {
        Fmt::Ids => {
            for r in &page {
                let _ = writeln!(s, "{}", r.p.topic);
            }
            return Ok(s);
        }
        Fmt::Json => {
            for r in &page {
                let comments: Vec<_> =
                    r.t.as_ref()
                        .filter(|_| co.wants())
                        .map(|t| co.select(t).iter().map(|x| co.json(x)).collect())
                        .unwrap_or_default();
                let mut v = json!({
                    "topic": r.p.topic, "exam": r.p.exam.name, "exam_id": r.p.exam.id, "year": r.p.exam.year,
                    "contest": r.p.exam.contest, "kind": r.p.exam.kind, "country": r.p.exam.country,
                    "heading": r.p.heading, "label": r.p.label, "pos": r.p.pos,
                });
                if !no_text {
                    v["text"] = json!(clip(&r.p.text, o.trunc));
                }
                if let Some(t) = &r.t {
                    v["comments_count"] = json!(t.comments);
                    v["views"] = json!(t.views);
                    v["tags"] = json!(t.tags);
                    v["source"] = json!(t.source);
                }
                if co.wants() {
                    v["comments"] = json!(comments);
                }
                let _ = writeln!(s, "{v}");
            }
            return Ok(s);
        }
        _ => {}
    }
    let mut last_exam = -1;
    for r in &page {
        if o.fmt == Fmt::Tsv {
            let (cc, vv, tg) = r.t.as_ref().map_or((String::new(), String::new(), String::new()), |t| {
                (t.comments.to_string(), t.views.to_string(), tag_list(&t.tags))
            });
            let _ = writeln!(
                s,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                r.p.topic,
                r.p.exam.year.map_or("?".into(), |y| y.to_string()),
                r.p.exam.name,
                r.p.label,
                cc,
                vv,
                tg
            );
            continue;
        }
        if r.p.exam.id != last_exam {
            let _ = writeln!(s, "## {} (cat {})", r.p.exam.name, r.p.exam.id);
            last_exam = r.p.exam.id;
        }
        let mut head = format!("#{}", r.p.topic);
        for part in [r.p.label.as_str(), r.p.heading.as_str()] {
            if !part.is_empty() {
                let _ = write!(head, " | {part}");
            }
        }
        if let Some(t) = &r.t {
            let _ = write!(head, " | {}", counters(t));
            if !t.tags.is_empty() {
                let _ = write!(head, " | {}", tag_list(&t.tags));
            }
        }
        let _ = writeln!(s, "{head}");
        if !no_text {
            let _ = writeln!(s, "{}", clip(&r.p.text, o.trunc));
        }
        if let (true, Some(t)) = (co.wants(), &r.t) {
            write_comments(&mut s, &co, t);
        }
    }
    if o.fmt != Fmt::Txt {
        return Ok(s);
    }
    let shown = page.len();
    let _ = write!(
        s,
        "-- {shown} of {total}{} problems, {n_exams} exams",
        if partial { "+" } else { "" }
    );
    if o.offset + shown < total || partial {
        let _ = write!(s, "; more: --offset {}", o.offset + shown);
    }
    if exams_total > exams.len() {
        let _ = write!(
            s,
            "; only newest {} of {exams_total} exams read (--max-exams or narrow -y/-C)",
            exams.len()
        );
    }
    if partial {
        let _ = write!(s, "; metadata scan stopped at {scanned} topics (--scan N to raise)");
    }
    s.push('\n');
    for w in warns.iter().take(5) {
        let _ = writeln!(s, "!! {w}");
    }
    if warns.len() > 5 {
        let _ = writeln!(s, "!! +{} more warnings", warns.len() - 5);
    }
    Ok(s)
}

fn shuffle<T>(v: &mut [T]) {
    let mut x = crate::util::now() as u64 ^ (std::process::id() as u64) << 20 | 1;
    for i in (1..v.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.swap(i, (x % (i as u64 + 1)) as usize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exam() -> Arc<Exam> {
        Arc::new(Exam {
            id: 1,
            name: "2025 IMO Shortlist".into(),
            year: Some(2025),
            contest: "IMO Shortlist".into(),
            kind: "intl".into(),
            country: String::new(),
        })
    }

    fn cat() -> api::Cat {
        serde_json::from_str(
            r#"{"category_id":1,"category_name":"x","category_type":"view_posts","items":[
            {"item_id":1,"item_text":"","item_type":"post","item_score":"0","item_level":"35","post_data":{"post_id":1,"post_type":"view_posts_text","topic_id":0,"post_canonical":"Algebra"}},
            {"item_id":2,"item_text":"A1","item_type":"post","post_data":{"post_id":2,"post_type":"forum","topic_id":100,"post_canonical":"Find $x$.\r\n\r\n\r\nDone"}},
            {"item_id":3,"item_text":"A2","item_type":"post","post_data":{"post_id":3,"post_type":"forum","topic_id":101,"post_canonical":"Prove it."}},
            {"item_id":4,"item_text":"","item_type":"post","post_data":{"post_id":4,"post_type":"view_posts_text","topic_id":0,"post_canonical":"Geometry"}},
            {"item_id":5,"item_text":"G1","item_type":"post","post_data":{"post_id":5,"post_type":"forum","topic_id":102,"post_canonical":"Circle."}},
            {"item_id":6,"item_text":"G2","item_type":"post_hidden","post_data":{"post_id":6,"post_type":"forum","topic_id":103,"post_canonical":"Hidden."}}
            ]}"#,
        )
        .unwrap()
    }

    fn filters(args: &str) -> Filters {
        let argv = crate::args::shell_split(args).unwrap();
        Filters::new(&crate::args::parse(&PROBLEMS, &argv).unwrap()).unwrap()
    }

    #[test]
    fn parses_exam() {
        let v = parse_exam(&cat(), &exam());
        let ids: Vec<(i64, &str, &str, usize, bool)> = v
            .iter()
            .map(|p| (p.topic, p.label.as_str(), p.heading.as_str(), p.pos, p.last_in_group))
            .collect();
        assert_eq!(
            ids,
            [
                (100, "A1", "Algebra", 1, false),
                (101, "A2", "Algebra", 2, true),
                (102, "G1", "Geometry", 3, true)
            ]
        );
        assert_eq!(v[0].text, "Find $x$.\n\nDone");
    }

    #[test]
    fn filters_select() {
        let v = parse_exam(&cat(), &exam());
        let pick = |f: &Filters| v.iter().filter(|p| f.cheap(p)).map(|p| p.topic).collect::<Vec<_>>();
        assert_eq!(pick(&filters("--hard")), [101, 102]);
        assert_eq!(pick(&filters("--pos 1,-1")), [100, 102]);
        assert_eq!(pick(&filters("--label ^G")), [102]);
        assert_eq!(pick(&filters("-k circle,prove")), [101, 102]);
        assert_eq!(pick(&filters("-k prove -k geometry")), [] as [i64; 0]);
        assert_eq!(pick(&filters("-k algebra")), [100, 101]);
    }

    #[test]
    fn topic_filters() {
        let t = Topic {
            comments: 12,
            views: 500,
            tags: vec!["Inversion".into(), "geometry".into()],
            ..Default::default()
        };
        assert!(filters("--min-comments 12 --tag inversion").topic_ok(&t));
        assert!(!filters("--min-comments 13").topic_ok(&t));
        assert!(!filters("--tag algebra --tag geometry").topic_ok(&t));
        assert!(filters("--tag algebra,geometry --max-comments 12 --min-views 500").topic_ok(&t));
    }
}
