use crate::api;
use crate::args::{f, Cmd, Flag, Parsed};
use crate::index::{self, Contest, Query};
use crate::net::Client;
use crate::out::{Fmt, Out};
use crate::util::{jstr, Res, YearSet};
use std::fmt::Write;

/// Flags selecting contests; shared by find / exams / problems.
pub const SELECT_FLAGS: &[Flag] = &[
    f!("country", 'C', "C,..", "country: ISO3 or name (china,IRN,'hong kong')"),
    f!("kind", 'K', "K,..", "intl|nat|tst|junior (nat=national olympiad)"),
    f!("id", "ID,..", "AoPS category ids (contest folder or exam), added to the selection"),
];

pub const YEARS_FLAG: &[Flag] = &[f!("years", 'y', "R", "years: 2024 | 2020-2024 | 2015,2019- | -2010")];

pub fn query_from(p: &Parsed, stdin: Option<&str>) -> Res<Query> {
    let sub = |v: &str| -> Res<Vec<String>> {
        let mut out = vec![];
        for x in crate::util::split_list(v) {
            if x == "-" {
                out.extend(stdin_items(stdin)?);
            } else {
                out.push(x);
            }
        }
        Ok(out)
    };
    let mut q = Query::default();
    let add = |q: &mut Query, x: String| match x.parse::<i64>() {
        Ok(id) if x.bytes().all(|b| b.is_ascii_digit()) => q.ids.push(id),
        _ => q.words.push(x),
    };
    let mut literal: Vec<&str> = vec![];
    for w in &p.pos {
        if w == "-" {
            for x in stdin_items(stdin)? {
                add(&mut q, x);
            }
        } else if w.bytes().all(|b| b.is_ascii_digit()) {
            q.ids.push(w.parse().map_err(|_| format!("bad id {w}"))?);
        } else {
            literal.push(w);
        }
    }
    if !literal.is_empty() {
        q.words.push(literal.join(" "));
    }
    for v in p.all("id") {
        for x in sub(v)? {
            q.ids.push(x.parse().map_err(|_| format!("bad --id '{x}'"))?);
        }
    }
    for v in p.all("country") {
        q.countries.extend(sub(v)?);
    }
    for v in p.all("kind") {
        q.kinds.extend(sub(v)?.into_iter().map(|k| k.to_lowercase()));
    }
    Ok(q)
}

pub fn years_from(p: &Parsed) -> Res<YearSet> {
    let mut ys = YearSet::default();
    for v in p.all("years") {
        ys.0.extend(YearSet::parse(v)?.0);
    }
    Ok(ys)
}

/// First column of each piped line; `-` without piped input is an error.
pub fn stdin_items(stdin: Option<&str>) -> Res<Vec<String>> {
    let s = stdin.ok_or("'-' needs piped input (e.g. `aops rank --fmt ids | aops problems -C - ...`)")?;
    let v: Vec<String> = s
        .lines()
        .filter_map(|l| l.split('\t').next())
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty() && !x.starts_with("--"))
        .collect();
    if v.is_empty() {
        return Err("piped input is empty (the upstream command produced nothing)".into());
    }
    Ok(v)
}

pub const FIND: Cmd = Cmd {
    name: "find",
    usage: "find [name words|alt|alt ...] [--country --kind]",
    about: "list contests (folders); name pattern = all words match, '|' = alternatives",
    flags: &[SELECT_FLAGS],
    examples: &["find --kind tst -C china,iran", "find 'imo|balkan' --kind intl", "find shortlist"],
};

pub async fn find(c: &Client, p: Parsed, stdin: Option<String>) -> Res<String> {
    let o = Out::new(&p, 40)?;
    let q = query_from(&p, stdin.as_deref())?;
    let list = index::find(c, &q).await?;
    Ok(render_contests(&list, &o))
}

fn render_contests(list: &[Contest], o: &Out) -> String {
    let total = list.len();
    let page: Vec<&Contest> = list.iter().skip(o.offset).take(o.limit).collect();
    let mut s = String::new();
    match o.fmt {
        Fmt::Count => return format!("{total}\n"),
        Fmt::Ids => {
            for x in &page {
                let _ = writeln!(s, "{}", x.id);
            }
            return s;
        }
        Fmt::Json => {
            for x in &page {
                let _ = writeln!(
                    s,
                    "{{\"id\":{},\"name\":{},\"kind\":\"{}\",\"country\":\"{}\",\"leaf\":{}}}",
                    x.id,
                    jstr(&x.name),
                    x.kind,
                    x.country,
                    x.leaf
                );
            }
            return s;
        }
        _ => {}
    }
    if o.fmt == Fmt::Txt {
        let _ = writeln!(s, "id\tkind\tctry\tname");
    }
    for x in &page {
        let _ = writeln!(s, "{}\t{}\t{}\t{}", x.id, x.kind, x.country, x.name);
    }
    if o.fmt == Fmt::Txt && total > o.offset + page.len() {
        let _ = writeln!(s, "…{} more (--offset {})", total - o.offset - page.len(), o.offset + page.len());
    }
    s
}

pub const EXAMS: Cmd = Cmd {
    name: "exams",
    usage: "exams <contest words|id ...> [--country --kind --years]",
    about: "list exam editions (year categories holding problems) of contests",
    flags: &[SELECT_FLAGS, YEARS_FLAG],
    examples: &["exams 'china tst' --years 2020-", "exams --kind tst -C iran,korea -y 2022-2025"],
};

pub async fn exams(c: &Client, p: Parsed, stdin: Option<String>) -> Res<String> {
    let o = Out::new(&p, 60)?;
    let q = query_from(&p, stdin.as_deref())?;
    if q.words.is_empty() && q.ids.is_empty() && q.countries.is_empty() && q.kinds.is_empty() {
        return Err("exams: give a contest name/id or --country/--kind (see: aops find)".into());
    }
    let contests = index::find(c, &q).await?;
    let list = index::exams(c, &contests, &years_from(&p)?).await?;
    let total = list.len();
    let page: Vec<_> = list.iter().skip(o.offset).take(o.limit).collect();
    let mut s = String::new();
    match o.fmt {
        Fmt::Count => return Ok(format!("{total}\n")),
        Fmt::Ids => {
            for e in &page {
                let _ = writeln!(s, "{}", e.id);
            }
            return Ok(s);
        }
        Fmt::Json => {
            for e in &page {
                let _ = writeln!(
                    s,
                    "{{\"id\":{},\"name\":{},\"year\":{},\"kind\":\"{}\",\"country\":\"{}\"}}",
                    e.id,
                    jstr(&e.name),
                    e.year.map_or("null".into(), |y| y.to_string()),
                    e.kind,
                    e.country
                );
            }
            return Ok(s);
        }
        _ => {}
    }
    if o.fmt == Fmt::Txt {
        let _ = writeln!(s, "id\tyear\tkind\tctry\tname");
    }
    for e in &page {
        let _ = writeln!(
            s,
            "{}\t{}\t{}\t{}\t{}",
            e.id,
            e.year.map_or("?".into(), |y| y.to_string()),
            e.kind,
            e.country,
            e.name
        );
    }
    if o.fmt == Fmt::Txt && total > o.offset + page.len() {
        let _ = writeln!(s, "…{} more (--offset {})", total - o.offset - page.len(), o.offset + page.len());
    }
    Ok(s)
}

pub const TREE: Cmd = Cmd {
    name: "tree",
    usage: "tree <category_id ...>",
    about: "raw children of any AoPS category (id type name); root of contest collections = 13",
    flags: &[],
    examples: &["tree 13", "tree 3223"],
};

pub async fn tree(c: &Client, p: Parsed, stdin: Option<String>) -> Res<String> {
    let o = Out::new(&p, 200)?;
    let mut ids: Vec<i64> = vec![];
    for w in &p.pos {
        for x in if w == "-" {
            stdin_items(stdin.as_deref())?
        } else {
            vec![w.clone()]
        } {
            ids.push(x.parse().map_err(|_| format!("tree: bad category id '{x}'"))?);
        }
    }
    if ids.is_empty() {
        return Err("tree: give category id(s); start at 13".into());
    }
    let cats = futures::future::try_join_all(ids.iter().map(|&id| api::category(c, id))).await?;
    let mut s = String::new();
    for cat in cats {
        let items: Vec<_> = cat.items.iter().skip(o.offset).take(o.limit).collect();
        match o.fmt {
            Fmt::Count => {
                let _ = writeln!(s, "{}", cat.items.len());
            }
            Fmt::Ids => {
                for i in &items {
                    let _ = writeln!(s, "{}", i.item_id);
                }
            }
            Fmt::Json => {
                for i in &items {
                    let _ = writeln!(
                        s,
                        "{{\"id\":{},\"type\":\"{}\",\"name\":{}}}",
                        i.item_id,
                        i.item_type,
                        jstr(&i.item_text)
                    );
                }
            }
            _ => {
                if o.fmt == Fmt::Txt {
                    let _ = writeln!(
                        s,
                        "# {} {} ({}) {} items",
                        cat.category_id,
                        cat.category_name,
                        cat.category_type,
                        cat.items.len()
                    );
                }
                for i in &items {
                    let _ = writeln!(s, "{}\t{}\t{}", i.item_id, i.item_type, i.item_text);
                }
                if o.fmt == Fmt::Txt && cat.items.len() > o.offset + items.len() {
                    let _ = writeln!(
                        s,
                        "…{} more (--offset {})",
                        cat.items.len() - o.offset - items.len(),
                        o.offset + items.len()
                    );
                }
            }
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(args: &str, stdin: Option<&str>) -> Res<Query> {
        let argv = crate::args::shell_split(args).unwrap();
        query_from(&crate::args::parse(&FIND, &argv).unwrap(), stdin)
    }

    #[test]
    fn positional_words_form_one_pattern() {
        let x = q("china tst 3282 'imo|balkan'", None).unwrap();
        assert_eq!(x.words, ["china tst imo|balkan"]);
        assert_eq!(x.ids, [3282]);
    }

    #[test]
    fn piped_items_split_ids_and_names() {
        let x = q("-", Some("3282\nHong Kong\n-- footer\n77\tx\n")).unwrap();
        assert_eq!(x.ids, [3282, 77]);
        assert_eq!(x.words, ["Hong Kong"]);
        let c = q("-C -", Some("China\nHong Kong\n")).unwrap();
        assert_eq!(c.countries, ["China", "Hong Kong"]);
        assert!(q("-C -", None).is_err());
    }
}
