use crate::args::{f, Cmd, Flag, Parsed};
use crate::countries;
use crate::net::{Client, DAY};
use crate::out::{Fmt, Out};
use crate::util::{jstr, now, unesc, Res, YearSet};
use futures::future::try_join_all;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Row {
    pub iso: String,
    pub name: String,
    pub rank: i64,
    pub total: i64,
    pub probs: Vec<i64>,
    pub medals: [i64; 4],
}

const BASE: &str = "https://www.imo-official.org/results/team/year";

fn cell_text(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    unesc(out.trim())
}

pub fn parse_table(html: &str) -> Vec<Row> {
    let Some(start) = html.find("<tbody") else {
        return vec![];
    };
    let body = &html[start..];
    let body = &body[..body.find("</tbody>").unwrap_or(body.len())];
    let mut rows = vec![];
    for tr in body.split("<tr").skip(1) {
        let tr = tr.split("</tr>").next().unwrap_or(tr);
        let Some(i) = tr.find("data-value=\"") else {
            continue;
        };
        let rest = &tr[i + 12..];
        let Some(j) = rest.find('"') else { continue };
        let name = unesc(&rest[..j]);
        let iso = tr
            .find("data-table__country-iso\">")
            .and_then(|k| tr[k + 25..].split('<').next())
            .unwrap_or("")
            .trim()
            .to_string();
        let nums: Vec<i64> = tr
            .split("<td class=\"data-table__num")
            .skip(1)
            .map(|c| {
                let inner = c.split_once('>').map_or("", |x| x.1);
                cell_text(inner.split("</td>").next().unwrap_or("")).parse().unwrap_or(0)
            })
            .collect();
        if nums.len() < 8 || iso.is_empty() {
            continue;
        }
        let mut medals = [0i64; 4];
        for (k, m) in medals.iter_mut().enumerate() {
            *m = nums.get(8 + k).copied().unwrap_or(0);
        }
        rows.push(Row {
            iso,
            name,
            rank: nums[0],
            total: nums[1],
            probs: nums[2..8].to_vec(),
            medals,
        });
    }
    rows
}

async fn year(c: &Client, y: i32) -> Res<Arc<Vec<Row>>> {
    let ttl = if y as i64 >= crate::util::civil_from_days(now() / 86400).0 {
        DAY
    } else {
        60 * DAY
    };
    c.cached(&format!("imo:{y}"), ttl, || async {
        let html = c.get_text(&format!("{BASE}/{y}/")).await?;
        let rows = parse_table(&html);
        if rows.is_empty() {
            return Err(format!("IMO {y}: no results table"));
        }
        Ok(rows)
    })
    .await
}

/// The three most recent editions that have published results.
async fn latest_years(c: &Client, n: usize) -> Res<Vec<i32>> {
    let cur = crate::util::civil_from_days(now() / 86400).0 as i32;
    let mut out = vec![];
    for y in (cur - 6..=cur).rev() {
        if out.len() == n {
            break;
        }
        match year(c, y).await {
            Ok(_) => out.push(y),
            // not published yet
            Err(e) if e.contains("HTTP 404") || e.contains("no results table") => {}
            Err(e) => return Err(e),
        }
    }
    if out.is_empty() {
        return Err("rank: could not load any IMO results from imo-official.org".into());
    }
    Ok(out)
}

const FLAGS: &[Flag] = &[
    f!("years", 'y', "R", "editions to aggregate (default: latest 3), e.g. 2022-2025"),
    f!("by", "K", "score (default: mean % of winner's points)|avgrank|pts|gold"),
    f!("country", 'C', "C,..", "only these countries (ISO3/name)"),
    f!("detail", "", "add per-problem point totals P1..P6"),
];

pub const RANK: Cmd = Cmd {
    name: "rank",
    usage: "rank [--years R] [--by K] [-n TOP]",
    about: "IMO team ranking over recent years (imo-official.org); --fmt ids prints AoPS country names to pipe into problems",
    flags: &[FLAGS],
    examples: &[
        "rank -n 10",
        "rank --years 2020-2026 --by gold -n 20",
        "rank -n 5 --fmt ids | problems --country - --kind tst --years 2024- --hard",
    ],
};

#[derive(Default)]
struct Agg {
    iso: String,
    name: String,
    per_year: Vec<(i32, i64, i64, i64)>, // year, rank, total, top
    pct: Vec<f64>,
    pts: i64,
    medals: [i64; 4],
    probs: [i64; 6],
    avg_rank: f64,
    score: f64,
}

pub async fn run(c: &Client, p: Parsed, stdin: Option<String>) -> Res<String> {
    let o = Out::new(&p, 15)?;
    let years: Vec<i32> = match p.get("years") {
        Some(y) => {
            let ys = YearSet::parse(y)?;
            let (lo, hi) = (
                ys.0.iter().map(|r| r.0).min().unwrap_or(0).max(1959),
                ys.0.iter().map(|r| r.1).max().unwrap_or(0).min(2100),
            );
            let mut v: Vec<i32> = (lo..=hi).filter(|y| ys.contains(*y)).collect();
            v.reverse();
            v
        }
        None => latest_years(c, 3).await?,
    };
    if years.is_empty() || years.len() > 30 {
        return Err("rank: --years must select 1..30 editions".into());
    }
    let tables = try_join_all(years.iter().map(|&y| year(c, y))).await?;
    let mut by: HashMap<String, Agg> = HashMap::new();
    for (&y, rows) in years.iter().zip(&tables) {
        let top = rows.iter().map(|r| r.total).max().unwrap_or(1).max(1);
        for r in rows.iter() {
            let a = by.entry(r.iso.clone()).or_insert_with(|| Agg {
                iso: r.iso.clone(),
                name: r.name.clone(),
                ..Default::default()
            });
            a.per_year.push((y, r.rank, r.total, top));
            a.pct.push(100.0 * r.total as f64 / top as f64);
            a.pts += r.total;
            for k in 0..4 {
                a.medals[k] += r.medals[k];
            }
            for k in 0..6 {
                a.probs[k] += r.probs.get(k).copied().unwrap_or(0);
            }
        }
    }
    let n_years = years.len() as f64;
    let mut v: Vec<Agg> = by.into_values().collect();
    for a in &mut v {
        a.score = a.pct.iter().sum::<f64>() / n_years;
        let worst = tables.iter().map(|t| t.len() as f64 + 1.0).sum::<f64>() / n_years;
        let missing = n_years - a.per_year.len() as f64;
        a.avg_rank = (a.per_year.iter().map(|x| x.1 as f64).sum::<f64>() + missing * worst) / n_years;
    }
    if !p.all("country").is_empty() {
        let mut want: Vec<String> = vec![];
        for cs in p.all("country") {
            for x in crate::util::split_list(cs) {
                want.extend(if x == "-" {
                    super::browse::stdin_items(stdin.as_deref())?
                } else {
                    vec![x]
                });
            }
        }
        // known countries resolve to their IMO code; others (e.g. Montenegro) match the IMO name
        let isos: Vec<&str> = want.iter().filter_map(|w| countries::resolve(w).ok()).collect();
        let names: Vec<String> = want
            .iter()
            .filter(|w| countries::resolve(w).is_err())
            .map(|w| w.to_lowercase())
            .collect();
        v.retain(|a| {
            isos.contains(&a.iso.as_str())
                || names
                    .iter()
                    .any(|n| a.iso.eq_ignore_ascii_case(n) || a.name.to_lowercase().contains(n.as_str()))
        });
        if v.is_empty() {
            return Err(format!("rank: no country matches {} in these editions", want.join(",")));
        }
    }
    v.sort_by(|a, b| a.iso.cmp(&b.iso));
    match p.get("by").unwrap_or("score") {
        "score" => v.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.avg_rank.total_cmp(&b.avg_rank))),
        "avgrank" => v.sort_by(|a, b| a.avg_rank.total_cmp(&b.avg_rank).then(b.score.total_cmp(&a.score))),
        "pts" => v.sort_by_key(|a| std::cmp::Reverse(a.pts)),
        "gold" => v.sort_by_key(|a| std::cmp::Reverse((a.medals[0], a.medals[1], a.medals[2]))),
        o => return Err(format!("bad --by '{o}' (score|avgrank|pts|gold)")),
    }
    let total = v.len();
    if o.fmt == Fmt::Count {
        return Ok(format!("{total}\n"));
    }
    let key_of = |a: &Agg| countries::key(&a.iso).map_or_else(|| a.name.clone(), str::to_string);
    let mut s = String::new();
    if o.fmt == Fmt::Txt {
        let ys: Vec<String> = years.iter().map(|y| y.to_string()).collect();
        let _ = writeln!(
            s,
            "IMO teams {} | score=mean % of winner's pts, avgrk=mean rank | G S B HM | rank by year",
            ys.join(",")
        );
    }
    let detail = p.flag("detail");
    for (i, a) in v.iter().skip(o.offset).take(o.limit).enumerate() {
        let n = o.offset + i + 1;
        match o.fmt {
            Fmt::Ids => {
                let _ = writeln!(s, "{}", key_of(a));
            }
            Fmt::Json => {
                let yrs: Vec<String> = a
                    .per_year
                    .iter()
                    .map(|(y, r, t, _)| format!("{{\"year\":{y},\"rank\":{r},\"pts\":{t}}}"))
                    .collect();
                let _ = writeln!(
                    s,
                    "{{\"n\":{n},\"iso\":\"{}\",\"name\":{},\"aops\":{},\"score\":{:.1},\"avg_rank\":{:.1},\"pts\":{},\"medals\":[{},{},{},{}],\"problems\":{:?},\"years\":[{}]}}",
                    a.iso, jstr(&a.name), jstr(&key_of(a)), a.score, a.avg_rank, a.pts, a.medals[0], a.medals[1], a.medals[2], a.medals[3], a.probs, yrs.join(",")
                );
            }
            _ => {
                let yr: Vec<String> = a
                    .per_year
                    .iter()
                    .map(|(y, r, t, _)| format!("{:02}:{}/{}", y % 100, r, t))
                    .collect();
                let sep = if o.fmt == Fmt::Tsv { "\t" } else { " " };
                let _ = write!(
                    s,
                    "{n}{sep}{}{sep}{}{sep}{:.1}{sep}{:.1}{sep}{}{sep}{}/{}/{}/{}{sep}{}",
                    a.iso,
                    key_of(a),
                    a.score,
                    a.avg_rank,
                    a.pts,
                    a.medals[0],
                    a.medals[1],
                    a.medals[2],
                    a.medals[3],
                    yr.join(",")
                );
                if detail {
                    let _ = write!(s, "{sep}P:{}", a.probs.iter().map(|x| x.to_string()).collect::<Vec<_>>().join("/"));
                }
                s.push('\n');
            }
        }
    }
    if o.fmt == Fmt::Txt && o.offset + o.limit < total {
        let _ = writeln!(s, "-- {} of {total} countries; more: --offset {}", o.limit, o.offset + o.limit);
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_row() {
        let html = r#"<table><tbody><tr><td data-value="People&#39;s Republic of China"><a><span class="data-table__country-full">x</span><span class="data-table__country-iso">CHN</span></a></td>
<td class="data-table__num data-table__section-start" data-value="1"> 1 </td><td class="data-table__num data-table__total-cell">232</td>
<td class="data-table__num">42</td><td class="data-table__num">35</td><td class="data-table__num">35</td><td class="data-table__num">42</td><td class="data-table__num">42</td><td class="data-table__num">36</td>
<td class="data-table__num data-table__medal-cell" data-value="6000000"> 6 </td><td class="data-table__num data-table__medal-cell"> </td><td class="data-table__num data-table__medal-cell"> </td><td class="data-table__num data-table__medal-cell"> </td>
<td class="data-table__leader-cell">A</td></tr></tbody></table>"#;
        let r = parse_table(html);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].iso, "CHN");
        assert_eq!(r[0].name, "People's Republic of China");
        assert_eq!((r[0].rank, r[0].total), (1, 232));
        assert_eq!(r[0].probs, vec![42, 35, 35, 42, 42, 36]);
        assert_eq!(r[0].medals, [6, 0, 0, 0]);
    }
}
