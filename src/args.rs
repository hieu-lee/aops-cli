use crate::util::Res;

pub struct Flag {
    pub long: &'static str,
    pub short: Option<char>,
    /// Value placeholder; empty = boolean switch.
    pub meta: &'static str,
    pub help: &'static str,
}

pub struct Cmd {
    pub name: &'static str,
    pub usage: &'static str,
    pub about: &'static str,
    pub flags: &'static [&'static [Flag]],
    pub examples: &'static [&'static str],
}

macro_rules! f {
    ($l:expr, $m:expr, $h:expr) => {
        Flag {
            long: $l,
            short: None,
            meta: $m,
            help: $h,
        }
    };
    ($l:expr, $s:expr, $m:expr, $h:expr) => {
        Flag {
            long: $l,
            short: Some($s),
            meta: $m,
            help: $h,
        }
    };
}
pub(crate) use f;

pub const GLOBAL: &[Flag] = &[
    f!("fmt", 'f', "F", "txt|json(nd)|ids|tsv|count (default txt)"),
    f!("limit", 'n', "N", "max results (default per cmd)"),
    f!("offset", "N", "skip first N results"),
    f!("trunc", 't', "N", "clip statements to N chars, 0=none (default 1500)"),
    f!("jobs", 'j', "N", "parallel requests (default 8, max 64)"),
    f!("no-cache", "", "bypass disk cache"),
    f!("refresh", "", "refetch and overwrite cache"),
    f!("help", 'h', "", "show help"),
];

#[derive(Default, Debug)]
pub struct Parsed {
    pub pos: Vec<String>,
    vals: Vec<(&'static str, String)>,
}

impl Parsed {
    /// Does any positional or selector value ask for piped input (`-`, possibly inside a comma list)?
    pub fn wants_stdin(&self) -> bool {
        self.pos.iter().any(|w| w == "-")
            || self
                .vals
                .iter()
                .any(|(k, v)| matches!(*k, "country" | "kind" | "id") && v.split(',').any(|x| x.trim() == "-"))
    }
    pub fn flag(&self, name: &str) -> bool {
        self.vals.iter().any(|(k, _)| *k == name)
    }
    pub fn get(&self, name: &str) -> Option<&str> {
        self.vals.iter().rev().find(|(k, _)| *k == name).map(|(_, v)| v.as_str())
    }
    pub fn all(&self, name: &str) -> Vec<&str> {
        self.vals.iter().filter(|(k, _)| *k == name).map(|(_, v)| v.as_str()).collect()
    }
    pub fn num<T: std::str::FromStr>(&self, name: &str) -> Res<Option<T>> {
        self.get(name)
            .map(|v| v.trim().parse::<T>().map_err(|_| format!("--{name}: bad number '{v}'")))
            .transpose()
    }
    pub fn limit(&self, default: usize) -> Res<usize> {
        Ok(self.num::<usize>("limit")?.unwrap_or(default))
    }
}

pub fn parse(cmd: &Cmd, argv: &[String]) -> Res<Parsed> {
    let mut p = Parsed::default();
    let all: Vec<&Flag> = cmd.flags.iter().flat_map(|g| g.iter()).chain(GLOBAL.iter()).collect();
    let mut i = 0;
    let mut only_pos = false;
    while i < argv.len() {
        let a = &argv[i];
        i += 1;
        if only_pos || a == "-" || !a.starts_with('-') || is_negative_number(a) {
            p.pos.push(a.clone());
            continue;
        }
        if a == "--" {
            only_pos = true;
            continue;
        }
        let (flag, inline): (&Flag, Option<String>) = if let Some(body) = a.strip_prefix("--") {
            let (name, val) = match body.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (body, None),
            };
            (resolve(&all, name, cmd)?, val)
        } else {
            let mut ch = a[1..].chars();
            let c = ch.next().unwrap();
            let rest: String = ch.collect();
            let fl = all
                .iter()
                .find(|f| f.short == Some(c))
                .copied()
                .ok_or_else(|| format!("{}: unknown flag -{c}\n{}", cmd.name, flag_names(&all)))?;
            (fl, if rest.is_empty() { None } else { Some(rest) })
        };
        if flag.meta.is_empty() {
            if inline.is_some() && a.starts_with("--") {
                return Err(format!("--{} takes no value", flag.long));
            }
            p.vals.push((flag.long, String::new()));
        } else {
            let v = match inline {
                Some(v) => v,
                None => {
                    let v = argv
                        .get(i)
                        .ok_or_else(|| format!("--{} needs a value ({})", flag.long, flag.meta))?;
                    i += 1;
                    v.clone()
                }
            };
            p.vals.push((flag.long, v));
        }
    }
    Ok(p)
}

fn is_negative_number(s: &str) -> bool {
    s.len() > 1 && s.starts_with('-') && s[1..].chars().all(|c| c.is_ascii_digit() || c == '.')
}

fn flag_names(all: &[&Flag]) -> String {
    format!(
        "flags: {}",
        all.iter().map(|f| format!("--{}", f.long)).collect::<Vec<_>>().join(" ")
    )
}

fn resolve<'a>(all: &[&'a Flag], name: &str, cmd: &Cmd) -> Res<&'a Flag> {
    if let Some(f) = all.iter().find(|f| f.long == name) {
        return Ok(f);
    }
    let pre: Vec<&&Flag> = all.iter().filter(|f| f.long.starts_with(name)).collect();
    match pre.as_slice() {
        [f] => Ok(f),
        [] => Err(format!("{}: unknown flag --{name}\n{}", cmd.name, flag_names(all))),
        many => Err(format!(
            "{}: ambiguous flag --{name}: {}",
            cmd.name,
            many.iter().map(|f| format!("--{}", f.long)).collect::<Vec<_>>().join(" ")
        )),
    }
}

pub fn cmd_help(cmd: &Cmd) -> String {
    let mut s = format!("aops {} — {}\nusage: aops {}\n", cmd.name, cmd.about, cmd.usage);
    for f in cmd.flags.iter().flat_map(|g| g.iter()).chain(GLOBAL.iter()) {
        let short = f.short.map(|c| format!("-{c},")).unwrap_or_default();
        let meta = if f.meta.is_empty() { String::new() } else { format!(" {}", f.meta) };
        s.push_str(&format!(" {short}--{}{meta}  {}\n", f.long, f.help));
    }
    for e in cmd.examples {
        s.push_str(&format!("e.g. aops {e}\n"));
    }
    s
}

/// Split a command line into words (supports '…', "…" and backslash escapes).
pub fn shell_split(line: &str) -> Res<Vec<String>> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut has = false;
    let mut q: Option<char> = None;
    let mut it = line.chars();
    while let Some(c) = it.next() {
        match (q, c) {
            (Some(qc), c) if c == qc => q = None,
            (Some('"'), '\\') => match it.next() {
                Some(n @ ('"' | '\\')) => cur.push(n),
                Some(n) => {
                    cur.push('\\');
                    cur.push(n)
                }
                None => cur.push('\\'),
            },
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                q = Some(c);
                has = true
            }
            (None, '\\') => {
                if let Some(n) = it.next() {
                    cur.push(n);
                    has = true
                }
            }
            (None, c) if c.is_whitespace() => {
                if has || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    has = false
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if q.is_some() {
        return Err(format!("unterminated quote in: {line}"));
    }
    if has || !cur.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    const C: Cmd = Cmd {
        name: "t",
        usage: "t",
        about: "",
        flags: &[&[f!("years", 'y', "R", ""), f!("min-comments", "N", ""), f!("hard", "", "")]],
        examples: &[],
    };
    fn a(s: &str) -> Vec<String> {
        shell_split(s).unwrap()
    }

    #[test]
    fn parses_flags() {
        let p = parse(&C, &a("x -y 2020 --min-c=5 --hard -n3 - y")).unwrap();
        assert_eq!(p.pos, ["x", "-", "y"]);
        assert_eq!(p.get("years"), Some("2020"));
        assert_eq!(p.num::<i64>("min-comments").unwrap(), Some(5));
        assert!(p.flag("hard"));
        assert_eq!(p.limit(9).unwrap(), 3);
    }

    #[test]
    fn rejects_bad() {
        assert!(parse(&C, &a("--nope")).is_err());
        assert!(parse(&C, &a("--years")).is_err());
        assert!(parse(&C, &a("--hard=1")).is_err());
        assert!(parse(&C, &a("--m 1")).is_ok());
    }

    #[test]
    fn splits_quotes() {
        assert_eq!(a(r#"search "a b" 'c d' e\ f """#), ["search", "a b", "c d", "e f", ""]);
        assert!(shell_split("a 'b").is_err());
    }

    #[test]
    fn negative_values() {
        let p = parse(&C, &a("--min-comments -1 -5")).unwrap();
        assert_eq!(p.get("min-comments"), Some("-1"));
        assert_eq!(p.pos, ["-5"]);
    }
}
