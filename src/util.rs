use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub type Res<T> = Result<T, String>;

static FAILED: AtomicBool = AtomicBool::new(false);

/// Mark the process as failed (exit status 1) while still printing partial results.
pub fn fail(msg: &str) {
    mark_failed();
    eprintln!("error: {msg}");
}

pub fn mark_failed() {
    FAILED.store(true, Ordering::Relaxed);
}

pub fn warn(msg: &str) {
    eprintln!("warn: {msg}");
}

pub fn failed() -> bool {
    FAILED.load(Ordering::Relaxed)
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

pub fn unesc(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let semi = rest.find(';').filter(|&j| j <= 8);
        let rep = semi.and_then(|j| {
            let ent = &rest[1..j];
            let r = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ => ent
                    .strip_prefix('#')
                    .and_then(|n| match n.strip_prefix(['x', 'X']) {
                        Some(h) => u32::from_str_radix(h, 16).ok(),
                        None => n.parse().ok(),
                    })
                    .and_then(char::from_u32),
            };
            r.map(|c| (c, j))
        });
        match rep {
            Some((c, j)) => {
                out.push(c);
                rest = &rest[j + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Inverse of `unesc` for the five basic entities.
pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Clip to `n` chars (0 = unlimited), marking how much was dropped.
pub fn clip(s: &str, n: usize) -> String {
    if n == 0 {
        return s.to_string();
    }
    match s.char_indices().nth(n) {
        None => s.to_string(),
        Some((i, _)) => format!("{}…[+{}]", &s[..i], s[i..].chars().count()),
    }
}

/// Normalise whitespace: CRLF, trailing spaces, 3+ newlines.
pub fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut nl = 0;
    for line in s.replace('\r', "").split('\n') {
        let line = line.trim_end();
        if line.is_empty() {
            nl += 1;
            if nl > 1 {
                continue;
            }
        } else {
            nl = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

pub fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn split_list(s: &str) -> Vec<String> {
    s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn ymd(ts: i64) -> String {
    let (y, m, d) = civil_from_days(ts.div_euclid(86400));
    format!("{y:04}-{m:02}-{d:02}")
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// "2024" | "2024-06" | "2024-06-15" -> [start, end) unix seconds (UTC).
pub fn parse_period(s: &str) -> Res<(i64, i64)> {
    let bad = || format!("bad date '{s}' (use YYYY, YYYY-MM or YYYY-MM-DD)");
    let p: Vec<i64> = s.split('-').map(|x| x.parse::<i64>().map_err(|_| bad())).collect::<Res<_>>()?;
    let ok = |m: i64, d: i64| (1..=12).contains(&m) && d >= 1 && d <= days_in_month(p[0], m);
    if !(1970..=9999).contains(&p[0]) {
        return Err(bad());
    }
    let (a, b) = match p.as_slice() {
        [y] => (days_from_civil(*y, 1, 1), days_from_civil(y + 1, 1, 1)),
        [y, m] if ok(*m, 1) => {
            let (ny, nm) = if *m == 12 { (y + 1, 1) } else { (*y, m + 1) };
            (days_from_civil(*y, *m, 1), days_from_civil(ny, nm, 1))
        }
        [y, m, d] if ok(*m, *d) => {
            let a = days_from_civil(*y, *m, *d);
            (a, a + 1)
        }
        _ => return Err(bad()),
    };
    Ok((a * 86400, b * 86400))
}

/// Inclusive year ranges: "2024", "2019-2024", "2022-", "-2010", "2015,2019-2021", "2019..2024".
#[derive(Clone, Default, Debug)]
pub struct YearSet(pub Vec<(i32, i32)>);

impl YearSet {
    pub fn parse(s: &str) -> Res<YearSet> {
        let mut v = vec![];
        for part in split_list(s) {
            let part = part.replace("..", "-");
            let bad = || format!("bad year range '{part}'");
            let (lo, hi) = match part.split_once('-') {
                None => {
                    let y: i32 = part.parse().map_err(|_| bad())?;
                    (y, y)
                }
                Some((a, b)) => (
                    if a.is_empty() { i32::MIN } else { a.parse().map_err(|_| bad())? },
                    if b.is_empty() { i32::MAX } else { b.parse().map_err(|_| bad())? },
                ),
            };
            v.push((lo.min(hi), lo.max(hi)));
        }
        Ok(YearSet(v))
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn contains(&self, y: i32) -> bool {
        self.0.iter().any(|&(a, b)| a <= y && y <= b)
    }
}

pub fn jstr(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities() {
        assert_eq!(
            unesc("Contests &amp; Programs &#39;x&#39; &lt;b&gt; a&b"),
            "Contests & Programs 'x' <b> a&b"
        );
        assert_eq!(unesc("&#x41;&bogus;"), "A&bogus;");
        for t in ["A &lt; B", "a & b <i> \"x\" 'y'", "&amp;lt;"] {
            assert_eq!(unesc(&esc(t)), t);
        }
    }

    #[test]
    fn clipping() {
        assert_eq!(clip("abcdef", 3), "abc…[+3]");
        assert_eq!(clip("abc", 3), "abc");
        assert_eq!(clip("αβγδ", 2), "αβ…[+2]");
        assert_eq!(clip("abcdef", 0), "abcdef");
    }

    #[test]
    fn dates() {
        assert_eq!(parse_period("1970").unwrap().0, 0);
        assert_eq!(parse_period("2024-02-29").unwrap(), (1709164800, 1709251200));
        assert_eq!(ymd(1709164800), "2024-02-29");
        assert_eq!(parse_period("2024-12").unwrap().1, parse_period("2025").unwrap().0);
        assert!(parse_period("2024-13").is_err());
        assert!(parse_period("2025-02-29").is_err());
        assert!(parse_period("2024-02-29").is_ok());
        assert!(parse_period("9223372036854775807").is_err());
    }

    #[test]
    fn years() {
        let y = YearSet::parse("2015,2019-2021,2024-").unwrap();
        assert!(y.contains(2015) && y.contains(2020) && y.contains(2030));
        assert!(!y.contains(2016) && !y.contains(2022));
        assert!(YearSet::parse("20x").is_err());
        assert!(YearSet::parse("2019..2020").unwrap().contains(2020));
    }

    #[test]
    fn tidy_ws() {
        assert_eq!(tidy("a  \r\n\n\n\nb \n"), "a\n\nb");
    }
}
