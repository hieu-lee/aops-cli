use crate::api::{self, Cat, Item};
use crate::countries;
use crate::net::{Client, DAY};
use crate::util::{Res, YearSet};
use futures::future::try_join_all;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

pub const INTL: i64 = 14;
pub const NATIONAL_AND_REGIONAL: i64 = 16;
pub const NATIONAL: i64 = 58;
pub const TST: i64 = 59;
pub const JUNIOR: i64 = 62;

pub const KINDS: &[&str] = &["intl", "nat", "tst", "junior"];

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Contest {
    pub id: i64,
    pub name: String,
    pub kind: String,
    /// ISO3 of the country when known.
    pub country: String,
    /// True when the node itself holds problems (no year folders below).
    pub leaf: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Exam {
    pub id: i64,
    pub name: String,
    pub year: Option<i32>,
    pub contest: String,
    pub kind: String,
    pub country: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Top {
    contests: Vec<Contest>,
    /// (country folder id, lowercase key)
    country_folders: Vec<(i64, String)>,
}

fn re_tst() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)team selection|\btsts?\b|training camp|selection test|\bimo selection|\bteam sel").unwrap())
}

fn re_junior() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)\bjunior|\bjbmo|\bijmo").unwrap())
}

fn re_year() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\b(1[89]\d\d|20[0-4]\d)\b").unwrap())
}

pub fn year_of(item: &Item) -> Option<i32> {
    let s = item.item_score;
    if (1800..2050).contains(&s) {
        return Some(s as i32);
    }
    re_year().captures(&item.item_text).and_then(|c| c[1].parse().ok())
}

fn classify(name: &str) -> &'static str {
    if re_tst().is_match(name) {
        "tst"
    } else if re_junior().is_match(name) {
        "junior"
    } else {
        "nat"
    }
}

fn is_container(it: &Item) -> bool {
    matches!(it.item_type.as_str(), "folder" | "view_posts")
}

fn contest_of(it: &Item, kind: &str, country: &str) -> Contest {
    let country = if country.is_empty() {
        countries::infer(&it.item_text).unwrap_or("")
    } else {
        country
    };
    Contest {
        id: it.item_id,
        name: it.item_text.clone(),
        kind: kind.into(),
        country: country.into(),
        leaf: it.item_type == "view_posts",
    }
}

async fn top(c: &Client) -> Res<std::sync::Arc<Top>> {
    c.cached("index:top3", 14 * DAY, || async {
        let cats = try_join_all([INTL, NATIONAL_AND_REGIONAL, NATIONAL, TST, JUNIOR].map(|id| api::category(c, id))).await?;
        let [intl, folders, nat, tst, jun] = &cats[..] else {
            return Err("index: bad category count".into());
        };
        let mut seen: HashMap<i64, Contest> = HashMap::new();
        let mut order: Vec<i64> = vec![];
        let mut put = |it: &Item, kind: &str, over: bool| {
            if !is_container(it) || it.item_text.is_empty() {
                return;
            }
            if over || !seen.contains_key(&it.item_id) {
                if !seen.contains_key(&it.item_id) {
                    order.push(it.item_id);
                }
                seen.insert(it.item_id, contest_of(it, kind, ""));
            }
        };
        for it in &intl.items {
            put(it, "intl", false);
        }
        for it in &nat.items {
            put(it, "nat", true);
        }
        for it in &jun.items {
            put(it, "junior", true);
        }
        for it in &tst.items {
            put(it, "tst", true);
        }
        let country_folders = folders
            .items
            .iter()
            .filter(|i| i.item_type == "folder" && i.item_text.ends_with(" Contests"))
            .map(|i| (i.item_id, i.item_text.trim_end_matches(" Contests").to_lowercase()))
            .collect();
        Ok(Top {
            contests: order.into_iter().filter_map(|id| seen.remove(&id)).collect(),
            country_folders,
        })
    })
    .await
}

async fn country_contests(c: &Client, iso: &str, tp: &Top) -> Res<Vec<Contest>> {
    let key = countries::key(iso).unwrap_or(iso).to_lowercase();
    let Some((fid, _)) = tp.country_folders.iter().find(|(_, k)| *k == key) else {
        return Ok(vec![]);
    };
    let cat: std::sync::Arc<Cat> = api::category(c, *fid).await?;
    Ok(cat
        .items
        .iter()
        .filter(|i| is_container(i) && !i.item_text.is_empty())
        .map(|i| contest_of(i, classify(&i.item_text), iso))
        .collect())
}

#[derive(Default, Clone)]
pub struct Query {
    pub words: Vec<String>,
    pub ids: Vec<i64>,
    pub countries: Vec<String>,
    pub kinds: Vec<String>,
}

/// Contest nodes matching all of: id list, name patterns, countries, kinds.
pub async fn find(c: &Client, q: &Query) -> Res<Vec<Contest>> {
    let selecting = !(q.words.is_empty() && q.countries.is_empty() && q.kinds.is_empty());
    if !selecting && !q.ids.is_empty() {
        return id_contests(c, &q.ids).await;
    }
    let tp = top(c).await?;
    let mut pool: Vec<Contest> = tp.contests.clone();
    let isos: Vec<&'static str> = q.countries.iter().map(|s| countries::resolve(s)).collect::<Res<_>>()?;
    if !isos.is_empty() {
        let extra = try_join_all(isos.iter().map(|iso| country_contests(c, iso, &tp))).await?;
        let mut have: HashSet<i64> = pool.iter().map(|x| x.id).collect();
        for x in extra.into_iter().flatten() {
            if have.insert(x.id) {
                pool.push(x);
            } else if let Some(p) = pool.iter_mut().find(|p| p.id == x.id) {
                if p.country.is_empty() {
                    p.country = x.country.clone();
                }
            }
        }
    }
    for k in &q.kinds {
        if !KINDS.contains(&k.as_str()) {
            return Err(format!("bad --kind '{k}' (use {})", KINDS.join("|")));
        }
    }
    // ids found in the pool keep their metadata; others are taken verbatim
    let mut out: Vec<Contest> = vec![];
    let mut missing: Vec<i64> = vec![];
    for &id in &q.ids {
        match pool.iter().find(|x| x.id == id) {
            Some(x) => out.push(x.clone()),
            None => missing.push(id),
        }
    }
    out.extend(id_contests(c, &missing).await?);
    if selecting || q.ids.is_empty() {
        pool.retain(|x| {
            (isos.is_empty() || isos.contains(&x.country.as_str()))
                && (q.kinds.is_empty()
                    || q.kinds.contains(&x.kind)
                    || (q.kinds.iter().any(|k| k == "junior") && re_junior().is_match(&x.name)))
        });
        let alts: Vec<&str> = q
            .words
            .iter()
            .flat_map(|w| w.split('|'))
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .collect();
        let mut picked: HashSet<i64> = HashSet::new();
        for alt in &alts {
            picked.extend(match_alt(&pool, alt));
        }
        for x in pool {
            if (alts.is_empty() || picked.contains(&x.id)) && !out.iter().any(|o| o.id == x.id) {
                out.push(x);
            }
        }
    }
    Ok(out)
}

/// Contest nodes for explicit category ids (any category), fetched in parallel.
async fn id_contests(c: &Client, ids: &[i64]) -> Res<Vec<Contest>> {
    let cats = try_join_all(ids.iter().map(|&id| api::category(c, id))).await?;
    Ok(ids
        .iter()
        .zip(cats)
        .map(|(&id, cat)| Contest {
            id,
            name: cat.category_name.clone(),
            kind: "-".into(),
            country: countries::infer(&cat.category_name).unwrap_or("").into(),
            leaf: cat.category_type == "view_posts",
        })
        .collect())
}

/// Common abbreviations -> contest name.
const ALIASES: &[(&str, &str)] = &[
    ("isl", "imo shortlist"),
    ("ill", "imo longlists"),
    ("bmo", "balkan mo"),
    ("jbmo", "junior balkan mo"),
    ("rmm", "romanian masters"),
    ("ibero", "iberoamerican"),
];

fn norm(s: &str) -> String {
    let mut out = String::new();
    for w in s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&w.to_lowercase());
    }
    out
}

/// One name pattern: exact (normalised) name match wins; otherwise every word must prefix a name word.
/// A trailing `*` skips the exact rule.
fn match_alt(pool: &[Contest], alt: &str) -> Vec<i64> {
    let (alt, fuzzy_only) = match alt.strip_suffix('*') {
        Some(a) => (a, true),
        None => (alt, false),
    };
    let mut q = norm(alt);
    if let Some((_, full)) = ALIASES.iter().find(|(a, _)| *a == q) {
        q = full.to_string();
    }
    if !fuzzy_only {
        let exact: Vec<i64> = pool.iter().filter(|x| norm(&x.name) == q).map(|x| x.id).collect();
        if !exact.is_empty() {
            return exact;
        }
    }
    let words: Vec<&str> = q.split(' ').collect();
    pool.iter()
        .filter(|x| {
            let n = norm(&x.name);
            let nw: Vec<&str> = n.split(' ').collect();
            words.iter().all(|w| nw.iter().any(|t| t.starts_with(w)))
        })
        .map(|x| x.id)
        .collect()
}

/// Expand contests into exam (leaf) categories, filtered by years. Folders are walked up to `depth` levels.
pub async fn exams(c: &Client, contests: &[Contest], years: &YearSet) -> Res<Vec<Exam>> {
    let lists = try_join_all(contests.iter().map(|k| walk(c, k, years))).await?;
    let mut v: Vec<Exam> = lists.into_iter().flatten().collect();
    let mut seen = HashSet::new();
    v.retain(|e| seen.insert(e.id));
    v.sort_by(|a, b| {
        b.year
            .cmp(&a.year)
            .then_with(|| a.contest.cmp(&b.contest))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(v)
}

async fn walk(c: &Client, k: &Contest, years: &YearSet) -> Res<Vec<Exam>> {
    let mk = |id: i64, name: &str, year: Option<i32>| Exam {
        id,
        name: name.to_string(),
        year,
        contest: k.name.clone(),
        kind: k.kind.clone(),
        country: k.country.clone(),
    };
    let year_ok = |y: Option<i32>| years.is_empty() || y.is_some_and(|y| years.contains(y));
    let mut out = vec![];
    if k.leaf {
        let y = re_year().captures(&k.name).and_then(|c| c[1].parse().ok());
        if year_ok(y) {
            out.push(mk(k.id, &k.name, y));
        }
        return Ok(out);
    }
    let mut frontier = vec![(k.id, re_year().captures(&k.name).and_then(|c| c[1].parse().ok()))];
    for _depth in 0..3 {
        let cats = try_join_all(frontier.iter().map(|(id, _)| api::category(c, *id))).await?;
        let mut next = vec![];
        for (cat, (_, inherited)) in cats.iter().zip(&frontier) {
            for it in &cat.items {
                let y = year_of(it).or(*inherited);
                match it.item_type.as_str() {
                    "view_posts" if year_ok(y) => out.push(mk(it.item_id, &it.item_text, y)),
                    "folder" if years.is_empty() || y.is_none() || year_ok(y) => next.push((it.item_id, y)),
                    _ => {}
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kinds() {
        assert_eq!(classify("China Team Selection Test"), "tst");
        assert_eq!(classify("Dutch IMO TST"), "tst");
        assert_eq!(classify("India IMO Training Camp"), "tst");
        assert_eq!(classify("Greece National Olympiad"), "nat");
        assert_eq!(classify("Junior Balkan MO"), "junior");
    }
    fn c(id: i64, name: &str) -> Contest {
        Contest {
            id,
            name: name.into(),
            kind: "intl".into(),
            country: String::new(),
            leaf: false,
        }
    }
    #[test]
    fn name_match() {
        let pool = vec![c(1, "IMO"), c(2, "IMO Shortlist"), c(3, "Simon Marais"), c(4, "Dutch IMO TST")];
        assert_eq!(match_alt(&pool, "imo"), [1]);
        assert_eq!(match_alt(&pool, "imo*"), [1, 2, 4]);
        assert_eq!(match_alt(&pool, "ISL"), [2]);
        assert_eq!(match_alt(&pool, "imo tst"), [4]);
        assert_eq!(match_alt(&pool, "mar"), [3]);
    }
    #[test]
    fn years_from_items() {
        let it = Item {
            item_text: "2019 China TST".into(),
            item_score: 0,
            ..Default::default()
        };
        assert_eq!(year_of(&it), Some(2019));
        let it = Item {
            item_text: "Test".into(),
            item_score: 2024,
            ..Default::default()
        };
        assert_eq!(year_of(&it), Some(2024));
    }
}
