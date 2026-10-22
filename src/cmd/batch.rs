use crate::args::{shell_split, Cmd};
use crate::util::Res;
use futures::future::join_all;
use std::future::Future;
use std::pin::Pin;

pub const BATCH: Cmd = Cmd {
    name: "batch",
    usage: "batch 'cmd ...' 'cmd ...'   (or one command per line on stdin; # comments)",
    about: "run many commands concurrently over one connection pool; outputs in order, each under '=== cmd'. 'a | b' feeds a's first column to b via '-'",
    flags: &[],
    examples: &["batch 'problems imo -y 2025 --hard' 'topic 3866863 -c 3' 'rank -n 5'"],
};

pub type Runner<'a> = &'a (dyn Fn(Vec<String>, Option<String>) -> Pin<Box<dyn Future<Output = Res<String>> + 'a>> + 'a);

/// Split a command line on unquoted '|' into pipeline stages.
pub fn stages(line: &str) -> Res<Vec<Vec<String>>> {
    let mut parts = vec![];
    let mut cur = String::new();
    let mut q: Option<char> = None;
    let mut esc = false;
    for ch in line.chars() {
        if esc {
            cur.push(ch);
            esc = false;
            continue;
        }
        match (q, ch) {
            (None, '\\') => {
                esc = true;
                cur.push(ch)
            }
            (Some(_), '\\') if q == Some('"') => {
                esc = true;
                cur.push(ch)
            }
            (None, '\'' | '"') => {
                q = Some(ch);
                cur.push(ch)
            }
            (Some(x), c) if c == x => {
                q = None;
                cur.push(c)
            }
            (None, '|') => parts.push(std::mem::take(&mut cur)),
            (_, c) => cur.push(c),
        }
    }
    parts.push(cur);
    parts.into_iter().map(|p| shell_split(&p)).collect()
}

pub async fn run<'a>(lines: Vec<String>, runner: Runner<'a>) -> Res<String> {
    let lines: Vec<String> = lines
        .into_iter()
        .flat_map(|l| l.lines().map(str::to_string).collect::<Vec<_>>())
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    if lines.is_empty() {
        return Err("batch: no commands (pass quoted commands or lines on stdin)".into());
    }
    let results = join_all(lines.iter().map(|line| async move {
        let st = stages(line)?;
        let mut out: Option<String> = None;
        for argv in st {
            if argv.is_empty() {
                return Err(format!("empty pipeline stage in: {line}"));
            }
            out = Some(runner(argv, out.take()).await?);
        }
        out.ok_or_else(|| format!("empty command: {line}"))
    }))
    .await;
    let mut s = String::new();
    for (line, r) in lines.iter().zip(results) {
        s.push_str(&format!("=== {line}\n"));
        match r {
            Ok(o) => {
                s.push_str(&o);
                if !o.ends_with('\n') {
                    s.push('\n');
                }
            }
            Err(e) => {
                crate::util::fail(&format!("batch '{line}': {e}"));
                s.push_str(&format!("!! {e}\n"))
            }
        }
    }
    Ok(s)
}
