# aops

A fast, token-frugal command line client for the [Art of Problem Solving community](https://artofproblemsolving.com/community), built for **LLM agents** that coach students towards the International Mathematical Olympiad (IMO). It is not meant for humans: output is compact, line oriented and designed to burn as few tokens as possible.

* Written in Rust: one static binary, HTTP/2 connection pooling, parallel requests, in-flight de-duplication.
* Disk cache (`~/.cache/aops`, override with `$AOPS_CACHE`) with per-kind TTLs, so repeated queries cost nothing.
* A self-describing `--help` (about 500 tokens for the whole tool) plus `aops help <cmd>` for flags.
* Sequences and pipelines in one process: `aops batch 'cmd' 'cmd' 'a | b'`.

## Install

```sh
cargo install --path .        # or: cargo build --release  -> target/release/aops
```

## What it can do

| command | purpose |
| --- | --- |
| `problems SEL [filters] [-c N]` | exam problems (LaTeX statements) from any contest, with filters and optional top comments |
| `topic ID\|URL.. [-c N]` | one or many topics: statement plus top comments, fetched in parallel |
| `search WORDS [--source --tags --title --forum --since ...]` | AoPS full-text search; result pages fetched in parallel |
| `tag TAG [--forum 6 --source RE ...]` | topics carrying a tag, newest activity first |
| `find`, `exams`, `tree` | discover contests, year editions, or browse the raw category tree |
| `rank` | IMO team ranking aggregated over the latest 3 editions (imo-official.org) |
| `batch` | run many commands concurrently; `a \| b` pipes ids from `a` into `b` through `-` |
| `cache info\|clear` | inspect or wipe the local cache |

### Selecting contests (`SEL`)

Contest words (all words must prefix a name word; an exact name wins; `a|b` gives alternatives; a trailing `*` forces fuzzy matching) or category ids, narrowed by

* `-C china,IRN,'hong kong'` country (ISO3 or name; ~100 countries known),
* `-K intl|nat|tst|junior` contest kind (`tst` = team selection tests / training camps, `nat` = national olympiads),
* `-y 2024`, `-y 2020-2024`, `-y 2015,2019-` years.

### Filtering problems

`-k kw` keyword (OR with `,`, repeat the flag for AND), `--label RE`, `--pos 3,6|-1`, `--hard` (last problem of every heading group: IMO P3/P6, ISL A7/C7/G8/N8, the hardest of each TST day), `--min-comments`, `--max-comments`, `--min-views`, `--tag`, `--sort comments|views|random|year`, `-n`/`--offset`. Metadata filters fetch topic data lazily, only as many topics as needed (`--scan` caps it).

Comments: `-c N` plus `--csort thanks|time|len|last`, `--cmin-thanks`, `--cmin-len`, `--cgrep RE`, `--cuser`, `--sol` (solution-like comments only).

### Output

`--fmt txt` (default, compact), `json` (one object per line), `ids` (one key per line, pipe-friendly), `tsv`, `count`. Statements are clipped with `-t N` (default 1500 chars, `0` = unlimited); clipped text ends with `…[+k]`. `c` is AoPS' `comment_count` (all posts including the statement) and `v` the view count.

### Examples

```sh
# the hardest problems of the last three IMOs, with the most-thanked comment each
aops problems imo -y 2024-2026 --hard -c 1

# which nations are strongest, then their hardest recent TST problems that got discussion
aops rank -n 5 --fmt ids | aops problems -C - -K tst -y 2024- --hard --min-comments 8 --sort comments -n 10

# find functional equation problems from shortlists with long threads
aops search 'functional equation' --source ISL --min-comments 20 --sort comments -n 10

# many things in one process
aops batch 'problems imo -y 2025 --hard' 'topic 3866863 -c 3 --sol' 'rank -n 8'
```

## Notes on the data source

AoPS exposes its community through an ajax endpoint that requires a short-lived anonymous session id, which `aops` obtains from the home page and caches. Please be considerate: the default parallelism is 8 (`-j`), responses are cached, and failures back off exponentially. The community API notice asks for data inquiries to `data-inquiries@artofproblemsolving.com`.

## Development

```sh
cargo test        # unit tests (parsers, filters, argument handling); no network needed
cargo clippy --all-targets
```
