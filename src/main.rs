mod api;
mod args;
mod cmd;
mod countries;
mod index;
mod net;
mod out;
mod util;

use args::{cmd_help, parse, Cmd, Parsed};
use cmd::{batch, browse, misc, problems, rank, search, topic};
use net::Client;
use std::future::Future;
use std::pin::Pin;
use util::Res;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

const CMD_CACHE: Cmd = Cmd {
    name: "cache",
    usage: "cache info|clear",
    about: "inspect or wipe the local cache",
    flags: &[],
    examples: &[],
};

fn all_cmds() -> Vec<&'static Cmd> {
    vec![
        &problems::PROBLEMS,
        &topic::TOPIC,
        &search::SEARCH,
        &search::TAG,
        &browse::FIND,
        &browse::EXAMS,
        &browse::TREE,
        &rank::RANK,
        &batch::BATCH,
        &CMD_CACHE,
    ]
}

fn find_cmd(name: &str) -> Option<&'static Cmd> {
    let alias = match name {
        "problem" | "p" | "probs" => "problems",
        "t" | "topics" | "thread" => "topic",
        "s" | "q" => "search",
        "tags" => "tag",
        "contests" | "ls" => "find",
        "exam" | "editions" => "exams",
        "ranking" | "nations" => "rank",
        n => n,
    };
    all_cmds().into_iter().find(|c| c.name == alias)
}

enum Plan {
    Text(String),
    Run(&'static Cmd, Parsed),
}

/// Resolve and parse a command line. `nested` = inside `batch`, where client-level flags are rejected.
fn plan(argv: &[String], nested: bool) -> Res<Plan> {
    let Some(name) = argv.first() else {
        return Err("empty command".into());
    };
    if name == "help" {
        return misc::help(&all_cmds(), argv.get(1).map(String::as_str)).map(Plan::Text);
    }
    let cmd = find_cmd(name).ok_or_else(|| format!("unknown command '{name}'. try: aops --help"))?;
    let p = parse(cmd, &argv[1..])?;
    if p.flag("help") {
        return Ok(Plan::Text(cmd_help(cmd)));
    }
    if matches!(cmd.name, "batch" | "cache") && ["fmt", "limit", "offset", "trunc"].iter().any(|f| p.get(f).is_some()) {
        return Err(format!(
            "{} takes no --fmt/-n/--offset/-t (for batch, put them inside each command)",
            cmd.name
        ));
    }
    if nested && (p.flag("no-cache") || p.flag("refresh") || p.get("jobs").is_some()) {
        return Err("--no-cache/--refresh/-j apply to the whole process: put them on `aops batch` itself".into());
    }
    Ok(Plan::Run(cmd, p))
}

/// Run one command; `stdin` is piped data for `-` arguments.
fn dispatch<'a>(c: &'a Client, argv: Vec<String>, stdin: Option<String>) -> Pin<Box<dyn Future<Output = Res<String>> + 'a>> {
    Box::pin(async move {
        match plan(&argv, true)? {
            Plan::Text(t) => Ok(t),
            Plan::Run(cmd, p) => exec(c, cmd, p, stdin).await,
        }
    })
}

async fn exec(c: &Client, cmd: &'static Cmd, p: Parsed, stdin: Option<String>) -> Res<String> {
    match cmd.name {
        "problems" => problems::run(c, p, stdin).await,
        "topic" => topic::run(c, p, stdin).await,
        "search" => search::search(c, p, stdin).await,
        "tag" => search::tag(c, p, stdin).await,
        "find" => browse::find(c, p, stdin).await,
        "exams" => browse::exams(c, p, stdin).await,
        "tree" => browse::tree(c, p, stdin).await,
        "rank" => rank::run(c, p, stdin).await,
        "cache" => misc::cache(c, &p.pos).await,
        "batch" => {
            let mut lines = p.pos.clone();
            if lines.is_empty() {
                lines = read_stdin().await?.lines().map(str::to_string).collect();
            }
            let runner = move |argv: Vec<String>, input: Option<String>| dispatch(c, argv, input);
            batch::run(lines, &runner).await
        }
        _ => unreachable!(),
    }
}

async fn read_stdin() -> Res<String> {
    use tokio::io::AsyncReadExt;
    let mut s = String::new();
    tokio::io::stdin().read_to_string(&mut s).await.map_err(|e| e.to_string())?;
    Ok(s)
}

/// Write to stdout; a closed pipe (`| head`) is fine, any other failure is an error.
fn emit(out: &str) -> bool {
    use std::io::Write;
    match std::io::stdout()
        .lock()
        .write_all(out.as_bytes())
        .and_then(|()| std::io::stdout().flush())
    {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => {
            eprintln!("error: cannot write output: {e}");
            false
        }
        _ => true,
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() || matches!(argv[0].as_str(), "-h" | "--help") {
        print!("{}", misc::HELP_TOP);
        return;
    }
    let (cmd, p) = match plan(&argv, false) {
        Ok(Plan::Text(t)) => std::process::exit(if emit(&t) { 0 } else { 1 }),
        Ok(Plan::Run(cmd, p)) => (cmd, p),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let code = rt.block_on(async move {
        let jobs = match p.num::<usize>("jobs") {
            Ok(j) => j
                .or_else(|| std::env::var("AOPS_JOBS").ok().and_then(|v| v.parse().ok()))
                .unwrap_or(8),
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        };
        let client = match Client::new(jobs, p.flag("no-cache"), p.flag("refresh")) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        };
        let stdin = if cmd.name != "batch" && p.wants_stdin() {
            read_stdin().await.ok()
        } else {
            None
        };
        match exec(&client, cmd, p, stdin).await {
            Ok(out) => i32::from(!emit(&out) || util::failed()),
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        }
    });
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> Vec<String> {
        args::shell_split(s).unwrap()
    }

    #[test]
    fn stdin_markers() {
        let w = |c: &str| matches!(plan(&a(c), false), Ok(Plan::Run(_, p)) if p.wants_stdin());
        assert!(w("find -") && w("find --country=-") && w("find -C-") && w("find -C China,-") && w("topic -"));
        assert!(!w("search --text -α") && !w("topic 1 --cgrep -") && !w("find -y 2015,2019-") && !w("find imo"));
    }

    #[test]
    fn batch_rejects_output_flags() {
        assert!(plan(&a("batch --fmt json x"), false).is_err());
        assert!(plan(&a("batch -j 2 x"), false).is_ok());
    }

    #[test]
    fn nested_client_flags_rejected() {
        assert!(plan(&a("rank --no-cache"), true).is_err());
        assert!(plan(&a("rank -j 4"), true).is_err());
        assert!(matches!(plan(&a("rank --no-cache"), false), Ok(Plan::Run(..))));
        assert!(matches!(plan(&a("help problems"), true), Ok(Plan::Text(_))));
        assert!(plan(&a("nope"), false).is_err());
    }
}
