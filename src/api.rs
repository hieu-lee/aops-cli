use crate::net::{Client, DAY, HOUR};
use crate::util::{esc, unesc, Res};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::sync::Arc;

fn de_num<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        Value::String(s) => s.trim().parse().unwrap_or(0),
        Value::Bool(b) => b as i64,
        _ => 0,
    })
}

/// Plain string (post text is stored unescaped by AoPS).
fn de_raw<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => s,
        Value::Null => String::new(),
        v => v.to_string(),
    })
}

fn ser_esc<S: serde::Serializer>(v: &str, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&esc(v))
}

/// HTML-escaped name/title fields; serialised back escaped so cache round-trips are lossless.
fn de_str<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => unesc(&s),
        Value::Null => String::new(),
        v => v.to_string(),
    })
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct PostData {
    #[serde(default, deserialize_with = "de_num")]
    pub post_id: i64,
    #[serde(default, deserialize_with = "de_num")]
    pub topic_id: i64,
    #[serde(default, deserialize_with = "de_str", serialize_with = "ser_esc")]
    pub post_type: String,
    #[serde(default, deserialize_with = "de_raw")]
    pub post_canonical: String,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Item {
    #[serde(default, deserialize_with = "de_num")]
    pub item_id: i64,
    #[serde(default, deserialize_with = "de_str", serialize_with = "ser_esc")]
    pub item_text: String,
    #[serde(default, deserialize_with = "de_str", serialize_with = "ser_esc")]
    pub item_type: String,
    #[serde(default, deserialize_with = "de_num")]
    pub item_score: i64,
    #[serde(default, deserialize_with = "de_num")]
    pub item_level: i64,
    #[serde(default)]
    pub post_data: Option<PostData>,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Cat {
    #[serde(default, deserialize_with = "de_num")]
    pub category_id: i64,
    #[serde(default, deserialize_with = "de_str", serialize_with = "ser_esc")]
    pub category_name: String,
    #[serde(default, deserialize_with = "de_str", serialize_with = "ser_esc")]
    pub category_type: String,
    #[serde(default)]
    pub items: Vec<Item>,
    #[serde(default)]
    pub no_more_items: bool,
}

#[derive(Deserialize)]
struct CatResp {
    category: Cat,
}

#[derive(Deserialize)]
struct MoreResp {
    #[serde(default)]
    items: Vec<Item>,
    #[serde(default)]
    no_more_items: bool,
}

/// Full child list of a category (folder or exam), following pagination.
pub async fn category(c: &Client, id: i64) -> Res<Arc<Cat>> {
    c.cached(&format!("cat:{id}"), 3 * DAY, || async {
        let r: CatResp = c.call("fetch_category_data", &[("category_id", id.to_string())]).await?;
        let mut cat = r.category;
        let mut more = !cat.no_more_items;
        while more {
            let Some(last) = cat.items.last() else { break };
            let before = cat.items.len();
            let r: MoreResp = c
                .call(
                    "fetch_more_items",
                    &[
                        ("category_id", id.to_string()),
                        ("last_item_score", last.item_score.to_string()),
                        ("last_item_level", last.item_level.to_string()),
                        ("last_item_text", last.item_text.clone()),
                        ("start_num", before.to_string()),
                        ("fetch_all", "1".into()),
                        ("log_visit", "0".into()),
                    ],
                )
                .await?;
            let have: std::collections::HashSet<i64> = cat.items.iter().map(|i| i.item_id).collect();
            cat.items.extend(r.items.into_iter().filter(|i| !have.contains(&i.item_id)));
            more = !r.no_more_items && cat.items.len() > before;
        }
        cat.no_more_items = true;
        Ok(cat)
    })
    .await
}

// ---------- topics ----------

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Post {
    pub n: i64,
    pub id: i64,
    pub user: String,
    pub thanks: i64,
    pub time: i64,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Topic {
    pub id: i64,
    pub title: String,
    pub forum: String,
    pub source: String,
    pub tags: Vec<String>,
    pub comments: i64,
    pub views: i64,
    pub created: i64,
    pub last: i64,
    pub n_posts: i64,
    pub posts: Vec<Post>,
}

impl Topic {
    pub fn complete(&self) -> bool {
        self.posts.len() as i64 >= self.n_posts
    }
    pub fn statement(&self) -> &str {
        self.posts
            .iter()
            .find(|p| p.n == 1)
            .or(self.posts.first())
            .map_or("", |p| p.text.as_str())
    }
}

#[derive(Deserialize, Default)]
struct TagW {
    #[serde(default, deserialize_with = "de_str")]
    tag_text: String,
}

#[derive(Deserialize, Default)]
struct PostW {
    #[serde(default, deserialize_with = "de_num")]
    post_id: i64,
    #[serde(default, deserialize_with = "de_num")]
    post_number: i64,
    #[serde(default, deserialize_with = "de_str")]
    username: String,
    #[serde(default, deserialize_with = "de_num")]
    thanks_received: i64,
    #[serde(default, deserialize_with = "de_num")]
    post_time: i64,
    #[serde(default, deserialize_with = "de_raw")]
    post_canonical: String,
    #[serde(default)]
    deleted: bool,
}

impl From<PostW> for Post {
    fn from(p: PostW) -> Post {
        Post {
            n: p.post_number,
            id: p.post_id,
            user: p.username,
            thanks: p.thanks_received,
            time: p.post_time,
            text: p.post_canonical,
        }
    }
}

#[derive(Deserialize, Default)]
pub struct TopicW {
    #[serde(default, deserialize_with = "de_num")]
    topic_id: i64,
    #[serde(default, deserialize_with = "de_str")]
    topic_title: String,
    #[serde(default, deserialize_with = "de_str")]
    category_name: String,
    #[serde(default, deserialize_with = "de_str")]
    source: String,
    #[serde(default)]
    tags: Vec<TagW>,
    #[serde(default, deserialize_with = "de_num")]
    comment_count: i64,
    #[serde(default, deserialize_with = "de_num")]
    num_views: i64,
    #[serde(default, deserialize_with = "de_num")]
    first_post_time: i64,
    #[serde(default, deserialize_with = "de_num")]
    last_post_time: i64,
    #[serde(default, deserialize_with = "de_num")]
    num_posts: i64,
    #[serde(default)]
    posts_data: Vec<PostW>,
}

impl From<TopicW> for Topic {
    fn from(t: TopicW) -> Topic {
        let posts: Vec<Post> = t.posts_data.into_iter().filter(|p| !p.deleted).map(Post::from).collect();
        let n_posts = if t.num_posts > 0 {
            t.num_posts
        } else {
            t.comment_count.max(posts.len() as i64)
        };
        Topic {
            id: t.topic_id,
            title: t.topic_title,
            forum: t.category_name,
            source: t.source,
            tags: t.tags.into_iter().map(|t| t.tag_text).collect(),
            comments: t.comment_count,
            views: t.num_views,
            created: t.first_post_time,
            last: t.last_post_time,
            n_posts,
            posts,
        }
    }
}

#[derive(Deserialize)]
struct TopicResp {
    topic: TopicW,
}

#[derive(Deserialize)]
struct PostsResp {
    #[serde(default)]
    posts: Vec<PostW>,
}

/// Topic metadata plus the posts AoPS returns inline (all of them for short threads).
pub async fn topic(c: &Client, id: i64) -> Res<Arc<Topic>> {
    c.cached(&format!("tp:{id}"), 6 * HOUR, || async {
        let r: TopicResp = c.call("fetch_topic", &[("topic_id", id.to_string())]).await?;
        let mut t = Topic::from(r.topic);
        if t.id == 0 {
            t.id = id;
        }
        Ok(t)
    })
    .await
}

/// Topic with every (non-deleted) post.
pub async fn topic_all(c: &Client, id: i64) -> Res<Arc<Topic>> {
    let t = topic(c, id).await?;
    if t.complete() {
        return Ok(t);
    }
    c.cached(&format!("tpa:{id}"), 6 * HOUR, || async {
        let mut t = (*t).clone();
        let mut posts: Vec<Post> = vec![];
        let mut seen: std::collections::HashSet<i64> = Default::default();
        let mut start = 1;
        for _ in 0..500 {
            let r: PostsResp = c
                .call(
                    "fetch_posts_for_topic",
                    &[
                        ("topic_id", id.to_string()),
                        ("direction", "forwards".into()),
                        ("start_post_id", "-1".into()),
                        ("start_post_num", start.to_string()),
                        ("show_from_time", "-1".into()),
                        ("num_to_fetch", "1000".into()),
                    ],
                )
                .await?;
            let last_raw = r.posts.iter().map(|p| p.post_number).max();
            for p in r.posts.into_iter().filter(|p| !p.deleted) {
                if seen.insert(p.post_id) {
                    posts.push(p.into());
                }
            }
            match last_raw {
                Some(l) if l >= start && (posts.len() as i64) < t.n_posts => start = l + 1,
                _ => break,
            }
        }
        if posts.is_empty() {
            return Ok(t);
        }
        posts.sort_by_key(|p| p.n);
        t.n_posts = t.n_posts.max(posts.len() as i64);
        t.posts = posts;
        Ok(t)
    })
    .await
}

// ---------- search / tag listings ----------

#[derive(Deserialize)]
struct TopicsResp {
    #[serde(default)]
    topics: Vec<TopicW>,
    #[serde(default)]
    no_more_topics: bool,
    #[serde(default)]
    num_search_results: i64,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Page {
    pub topics: Vec<Topic>,
    pub more: bool,
}

pub const SEARCH_PAGE: usize = 15;

/// One search result page at `offset` (multiple of 15). `settings` is the AoPS search_settings JSON.
pub async fn search(c: &Client, settings: &str, offset: usize) -> Res<Arc<Page>> {
    c.cached(&format!("srch:{settings}:{offset}"), HOUR, || async {
        let r: TopicsResp = c
            .call(
                "fetch_topics",
                &[
                    ("category_type", "search".into()),
                    ("search_settings", settings.to_string()),
                    ("start_search_id", offset.to_string()),
                    ("fetch_before", "99999999999".into()),
                    ("log_visit", "0".into()),
                    ("user_id", "0".into()),
                    ("fetch_archived", "0".into()),
                    ("fetch_announcements", "0".into()),
                ],
            )
            .await?;
        let more = !r.no_more_topics && r.num_search_results > 0;
        Ok(Page {
            topics: r.topics.into_iter().map(Topic::from).collect(),
            more,
        })
    })
    .await
}

/// Topics of a forum carrying `tag_id`, most recently active first, older than `before`.
pub async fn forum_tag(c: &Client, forum: i64, tag_id: i64, before: i64) -> Res<Arc<Page>> {
    c.cached(&format!("ftag:{forum}:{tag_id}:{before}"), HOUR, || async {
        let r: TopicsResp = c
            .call(
                "fetch_topics",
                &[
                    ("category_type", "view_topics".into()),
                    ("category_id", forum.to_string()),
                    ("required_tag", format!("{forum}_{tag_id}")),
                    ("fetch_before", before.to_string()),
                    ("log_visit", "0".into()),
                    ("user_id", "0".into()),
                    ("fetch_archived", "0".into()),
                    ("fetch_announcements", "0".into()),
                ],
            )
            .await?;
        let more = !r.no_more_topics && !r.topics.is_empty();
        Ok(Page {
            topics: r.topics.into_iter().map(Topic::from).collect(),
            more,
        })
    })
    .await
}

#[derive(Deserialize)]
struct TagIdResp {
    #[serde(default, deserialize_with = "de_num")]
    tag_id: i64,
}

pub async fn tag_id(c: &Client, text: &str) -> Res<i64> {
    let id = c
        .cached(&format!("tagid:{}", text.to_lowercase()), 30 * DAY, || async {
            let r: TagIdResp = c.call("fetch_tag_id", &[("tag_text", text.to_string())]).await?;
            if r.tag_id <= 0 {
                return Err(format!("unknown tag '{text}'"));
            }
            Ok(r.tag_id)
        })
        .await?;
    Ok(*id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_from_wire() {
        let w: TopicW = serde_json::from_str(
            r#"{"topic_id":"7","topic_title":"A &amp; B","category_name":"High School Olympiads","source":"ISL 2025 A1","tags":[{"tag_id":1,"tag_text":"algebra"}],
            "comment_count":"3","num_views":10,"first_post_time":5,"last_post_time":9,"num_posts":3,
            "posts_data":[{"post_id":1,"post_number":1,"username":"u","thanks_received":2,"post_time":5,"post_canonical":"S"},
                          {"post_id":2,"post_number":2,"deleted":true,"post_canonical":"gone"}]}"#,
        )
        .unwrap();
        let t = Topic::from(w);
        assert_eq!((t.id, t.title.as_str(), t.comments, t.views), (7, "A & B", 3, 10));
        assert_eq!(t.tags, ["algebra"]);
        assert_eq!(t.posts.len(), 1);
        assert!(!t.complete());
        assert_eq!(t.statement(), "S");
    }

    #[test]
    fn statements_survive_cache_roundtrip() {
        let it: Item =
            serde_json::from_str(r#"{"item_text":"A &amp; B","post_data":{"topic_id":1,"post_canonical":"if $a &lt; b$ &amp;amp; x"}}"#)
                .unwrap();
        assert_eq!(it.item_text, "A & B");
        let again: Item = serde_json::from_str(&serde_json::to_string(&it).unwrap()).unwrap();
        assert_eq!(again.item_text, "A & B");
        let tricky: Item = serde_json::from_str(r#"{"item_text":"A &amp;lt; B"}"#).unwrap();
        assert_eq!(tricky.item_text, "A &lt; B");
        let tricky2: Item = serde_json::from_str(&serde_json::to_string(&tricky).unwrap()).unwrap();
        assert_eq!(tricky2.item_text, "A &lt; B");
        assert_eq!(again.post_data.unwrap().post_canonical, "if $a &lt; b$ &amp;amp; x");
    }
}
