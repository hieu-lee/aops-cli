use crate::args::{cmd_help, Cmd};
use crate::net::Client;
use crate::util::Res;
use std::path::Path;

pub const HELP_TOP: &str = "\
aops — AoPS community CLI for LLMs (IMO prep). Output is compact text; --fmt json|ids|tsv|count.
CMDS (each has --help; `aops help all` = every flag):
 problems SEL [filters] [-c N]  exam problems: LaTeX statement + optional top comments
 topic ID|URL.. [-c N]          one/many topics: statement + comments (parallel)
 search WORDS [--source/--tags/--title/--forum/--since..]  full-text search, parallel pages
 tag TAG [--forum 6 --source RE ..]  topics by tag, newest activity first
 find [words] / exams SEL / tree ID  discover contests, editions, raw AoPS category tree (root 13)
 rank [-n 15] [--years]         IMO nations ranking, last 3 editions (imo-official.org)
 batch 'cmd' 'cmd' ..           run many commands in parallel (also stdin lines); 'a | b' pipes ids
 cache info|clear               local cache (~/.cache/aops; $AOPS_CACHE)
SEL = contest words (all must match; a|b = alternatives) or category ids, narrowed by:
 -C country (ISO3|name,..)  -K intl|nat|tst|junior  -y years (2024|2020-2024|2015,2019-)
FILTERS: -k kw (OR with ',', repeat=AND) --label RE --pos 3,6|-1 --hard (last of each group: P3/P6, A7..)
 --min-comments N --max-comments N --min-views N --tag T --sort comments|views|random|year
 -n N limit(default 20) --offset N  -t N clip chars  -c N comments (--csort thanks|time|len|last --cmin-thanks --sol)
 -j N parallelism(8) --no-cache --refresh
COMMENTS = AoPS comment_count (all posts incl. statement), v = views.
EX: aops rank -n 8 --fmt ids | aops problems --country - -K tst -y 2024- --hard --min-comments 8 -c 2
    aops batch 'problems imo -y 2025 --hard' 'search \"functional equation\" --source ISL -n 5 --sort comments'
";

pub fn help(cmds: &[&Cmd], topic: Option<&str>) -> Res<String> {
    match topic {
        None => Ok(HELP_TOP.to_string()),
        Some("all") => Ok(format!(
            "{HELP_TOP}\n{}",
            cmds.iter().map(|c| cmd_help(c)).collect::<Vec<_>>().join("\n")
        )),
        Some(t) => cmds.iter().find(|c| c.name == t).map(|c| cmd_help(c)).ok_or_else(|| {
            format!(
                "no such command '{t}'. commands: {}",
                cmds.iter().map(|c| c.name).collect::<Vec<_>>().join(" ")
            )
        }),
    }
}

fn dir_stats(p: &Path) -> (u64, u64) {
    let (mut n, mut bytes) = (0, 0);
    let Ok(rd) = std::fs::read_dir(p) else {
        return (0, 0);
    };
    for e in rd.flatten() {
        let path = e.path();
        if path.is_dir() {
            let (a, b) = dir_stats(&path);
            n += a;
            bytes += b;
        } else if let Ok(m) = e.metadata() {
            n += 1;
            bytes += m.len();
        }
    }
    (n, bytes)
}

pub async fn cache(c: &Client, pos: &[String]) -> Res<String> {
    let dir = c.cache_path();
    match pos.first().map(String::as_str) {
        None | Some("info") => {
            let (n, b) = dir_stats(dir);
            Ok(format!("{} {} files {} KB\n", dir.display(), n, b / 1024))
        }
        Some("clear") => {
            if !dir.join(crate::net::MARKER).is_file() {
                return Ok("cache empty\n".into());
            }
            let mut n = 0;
            let Ok(rd) = std::fs::read_dir(dir) else {
                return Ok("cache empty\n".into());
            };
            for e in rd.flatten() {
                let (name, path) = (e.file_name().to_string_lossy().into_owned(), e.path());
                let Ok(ty) = e.file_type() else { continue };
                if ty.is_dir() && name.len() == 2 && name.bytes().all(|b| b.is_ascii_hexdigit()) {
                    for f in std::fs::read_dir(&path).into_iter().flatten().flatten() {
                        let fname = f.file_name().to_string_lossy().into_owned();
                        let ours = f.file_type().is_ok_and(|t| t.is_file())
                            && fname.split_once('.').is_some_and(|(stem, ext)| {
                                stem.len() == 16 && stem.bytes().all(|b| b.is_ascii_hexdigit()) && (ext == "json" || ext.starts_with("tmp"))
                            });
                        if ours && std::fs::remove_file(f.path()).is_ok() {
                            n += 1;
                        }
                    }
                    let _ = std::fs::remove_dir(&path);
                } else if ty.is_file() && name == "session" {
                    let _ = std::fs::remove_file(path);
                }
            }
            Ok(format!("cleared {n} files\n"))
        }
        Some(o) => Err(format!("cache: unknown action '{o}' (info|clear)")),
    }
}
