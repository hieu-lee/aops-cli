use crate::util::{now, Res};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::value::RawValue;
use std::{
    any::Any,
    collections::HashMap,
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{OnceCell, Semaphore};

const AJAX: &str = "https://artofproblemsolving.com/m/community/ajax.php/";
const HOME: &str = "https://artofproblemsolving.com/";
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
pub const MARKER: &str = ".aops-cache";
const SESSION_TTL: i64 = 6 * 3600;
const ATTEMPTS: usize = 6;

pub const HOUR: u64 = 3600;
pub const DAY: u64 = 24 * HOUR;

type Shared = Arc<dyn Any + Send + Sync>;
type Cell = Arc<OnceCell<Result<Shared, String>>>;

pub struct Client {
    http: reqwest::Client,
    sem: Semaphore,
    session: tokio::sync::Mutex<Option<String>>,
    dir: PathBuf,
    no_cache: bool,
    refresh: bool,
    mem: Mutex<HashMap<String, Cell>>,
    marked: std::sync::atomic::AtomicBool,
    service_retry_at_ms: std::sync::atomic::AtomicU64,
}

#[derive(Deserialize)]
struct Env<'a> {
    #[serde(borrow)]
    response: Option<&'a RawValue>,
    error_code: Option<String>,
    error_msg: Option<serde_json::Value>,
}

fn cache_dir() -> PathBuf {
    if let Ok(d) = std::env::var("AOPS_CACHE") {
        return d.into();
    }
    if let Ok(d) = std::env::var("XDG_CACHE_HOME") {
        return PathBuf::from(d).join("aops");
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".cache/aops")
}

impl Client {
    pub fn new(jobs: usize, no_cache: bool, refresh: bool) -> Res<Client> {
        let http = reqwest::Client::builder()
            .user_agent(UA)
            .gzip(true)
            .brotli(true)
            .tcp_nodelay(true)
            .pool_max_idle_per_host(64)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(45))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Client {
            http,
            sem: Semaphore::new(jobs.clamp(1, 64)),
            session: Default::default(),
            dir: cache_dir(),
            no_cache,
            refresh,
            mem: Default::default(),
            marked: Default::default(),
            service_retry_at_ms: Default::default(),
        })
    }

    pub fn cache_path(&self) -> &PathBuf {
        &self.dir
    }

    /// Drop a marker so `cache clear` only ever deletes directories aops created.
    async fn mark_dir(&self) {
        if !self.marked.swap(true, std::sync::atomic::Ordering::Relaxed) {
            let _ = tokio::fs::create_dir_all(&self.dir).await;
            let _ = tokio::fs::write(self.dir.join(MARKER), "aops cache\n").await;
        }
    }

    fn file_for(&self, key: &str) -> PathBuf {
        let h = format!("{:016x}", crate::util::fnv(key));
        self.dir.join(&h[..2]).join(format!("{h}.json"))
    }

    async fn disk_get<T: DeserializeOwned>(&self, key: &str, ttl: u64) -> Option<T> {
        let path = self.file_for(key);
        let age = tokio::fs::metadata(&path).await.ok()?.modified().ok()?.elapsed().ok()?;
        if age.as_secs() > ttl {
            return None;
        }
        let text = tokio::fs::read_to_string(&path).await.ok()?;
        let (k, body) = text.split_once('\n')?;
        if k != key {
            return None;
        }
        serde_json::from_str(body).ok()
    }

    async fn disk_put<T: Serialize>(&self, key: &str, v: &T) {
        if self.no_cache {
            return;
        }
        let path = self.file_for(key);
        let Ok(body) = serde_json::to_string(v) else {
            return;
        };
        self.mark_dir().await;
        if let Some(parent) = path.parent() {
            if tokio::fs::create_dir_all(parent).await.is_err() {
                return;
            }
        }
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        let data = format!("{key}\n{body}");
        if tokio::fs::write(&tmp, data).await.is_ok() && tokio::fs::rename(&tmp, &path).await.is_err() {
            let _ = tokio::fs::remove_file(&tmp).await;
        }
    }

    /// Memoised (per process, in-flight deduplicated) + disk cached computation.
    pub async fn cached<T, F, Fut>(&self, key: &str, ttl: u64, f: F) -> Res<Arc<T>>
    where
        T: Serialize + DeserializeOwned + Send + Sync + 'static,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Res<T>>,
    {
        let cell = self.mem.lock().unwrap().entry(key.to_string()).or_default().clone();
        let r = cell
            .get_or_init(|| async {
                if !self.no_cache && !self.refresh {
                    if let Some(v) = self.disk_get::<T>(key, ttl).await {
                        return Ok(Arc::new(v) as Shared);
                    }
                }
                let v = f().await?;
                self.disk_put(key, &v).await;
                Ok(Arc::new(v) as Shared)
            })
            .await;
        match r {
            Ok(a) => a.clone().downcast::<T>().map_err(|_| format!("cache type clash for {key}")),
            Err(e) => Err(e.clone()),
        }
    }

    async fn fresh_session(&self, bad: Option<&str>) -> Res<String> {
        let mut g = self.session.lock().await;
        if let Some(s) = g.as_ref() {
            if Some(s.as_str()) != bad {
                return Ok(s.clone());
            }
        }
        let file = self.dir.join("session");
        if bad.is_none() && !self.no_cache {
            if let Ok(t) = tokio::fs::read_to_string(&file).await {
                if let Some((ts, id)) = t.trim().split_once(' ') {
                    if ts.parse::<i64>().is_ok_and(|ts| now() - ts < SESSION_TTL) && id.len() == 32 {
                        *g = Some(id.to_string());
                        return Ok(id.to_string());
                    }
                }
            }
        }
        let mut last = String::new();
        for n in 0..3u64 {
            let r = self.http.get(HOME).send().await;
            match r {
                Ok(resp) if resp.status().is_success() => {
                    let body = resp.text().await.map_err(|e| e.to_string())?;
                    if let Some(id) = find_session(&body) {
                        if !self.no_cache {
                            self.mark_dir().await;
                            let _ = tokio::fs::write(&file, format!("{} {id}", now())).await;
                        }
                        *g = Some(id.clone());
                        return Ok(id);
                    }
                    last = "no session id on home page".into();
                }
                Ok(resp) => last = format!("home page HTTP {}", resp.status()),
                Err(e) => last = e.to_string(),
            }
            tokio::time::sleep(Duration::from_millis(400 << n)).await;
        }
        Err(format!("cannot get AoPS session: {last}"))
    }

    /// POST an AoPS community ajax action; returns the typed `response` payload.
    pub async fn call<T: DeserializeOwned>(&self, action: &str, params: &[(&str, String)]) -> Res<T> {
        let mut last = String::new();
        let mut bad_session: Option<String> = None;
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                if std::env::var_os("AOPS_DEBUG").is_some() {
                    eprintln!("[retry {attempt}] {action}: {last}");
                }
                let ms = 300u64 << (attempt - 1).min(4);
                tokio::time::sleep(Duration::from_millis(ms + crate::util::now() as u64 % 97)).await;
            }
            let sid = self.fresh_session(bad_session.as_deref()).await?;
            let mut form: Vec<(&str, &str)> = vec![
                ("a", action),
                ("aops_session_id", &sid),
                ("aops_logged_in", "false"),
                ("aops_user_id", "1"),
            ];
            form.extend(params.iter().map(|(k, v)| (*k, v.as_str())));
            let permit = self.sem.acquire().await.map_err(|e| e.to_string())?;
            self.wait_for_service().await;
            let resp = self.http.post(AJAX).form(&form).send().await;
            let (status, body, outage, retry_after) = match resp {
                Ok(r) => {
                    let st = r.status();
                    let challenge = r
                        .headers()
                        .get("cf-mitigated")
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|v| v.eq_ignore_ascii_case("challenge"));
                    let outage = st.is_server_error() && !challenge;
                    let retry_after = outage
                        .then(|| {
                            r.headers()
                                .get(reqwest::header::RETRY_AFTER)?
                                .to_str()
                                .ok()?
                                .trim()
                                .parse::<u64>()
                                .ok()
                        })
                        .flatten();
                    (st, r.bytes().await.map_err(|e| e.to_string()), outage, retry_after)
                }
                Err(e) => {
                    last = e.to_string();
                    continue;
                }
            };
            let body = match body {
                Ok(b) => b,
                Err(e) => {
                    last = e;
                    continue;
                }
            };
            if outage && body.first() == Some(&b'<') {
                let delay = retry_after.map_or(2000u64 << attempt.min(4), |secs| secs.min(600) * 1000);
                self.service_retry_at_ms
                    .fetch_max(now_ms() + delay, std::sync::atomic::Ordering::Relaxed);
            }
            drop(permit);
            if !status.is_success() || body.first() == Some(&b'<') {
                last = format!(
                    "HTTP {status}{}",
                    if body.first() == Some(&b'<') { " (html/challenge)" } else { "" }
                );
                continue;
            }
            let env: Env = serde_json::from_slice(&body).map_err(|e| format!("bad JSON from AoPS: {e}"))?;
            if let Some(code) = env.error_code.as_deref().filter(|c| !c.is_empty()) {
                let msg = match &env.error_msg {
                    Some(serde_json::Value::String(s)) if !s.is_empty() => format!(" {s}"),
                    _ => String::new(),
                };
                if code.starts_with("E_INVALID_SESSION") {
                    bad_session = Some(sid);
                    last = code.to_string();
                    continue;
                }
                let up = code.to_ascii_uppercase();
                if ["INACTIVE", "BUSY", "TIMEOUT", "UNAVAILABLE"].iter().any(|w| up.contains(w)) {
                    last = format!("{code}{msg}");
                    continue;
                }
                return Err(format!("{action}: {code}{msg}"));
            }
            let raw = env.response.ok_or("AoPS reply without response")?;
            return serde_json::from_str(raw.get()).map_err(|e| format!("{action}: unexpected payload: {e}"));
        }
        Err(format!("{action}: failed after {ATTEMPTS} attempts: {last}"))
    }

    async fn wait_for_service(&self) {
        loop {
            let until = self.service_retry_at_ms.load(std::sync::atomic::Ordering::Relaxed);
            let now = now_ms();
            if until <= now {
                return;
            }
            tokio::time::sleep(Duration::from_millis(until - now)).await;
        }
    }

    /// Plain GET returning body text (used for IMO official site).
    pub async fn get_text(&self, url: &str) -> Res<String> {
        let mut last = String::new();
        for attempt in 0..3u64 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(400 << attempt)).await;
            }
            let _p = self.sem.acquire().await.map_err(|e| e.to_string())?;
            match self.http.get(url).send().await {
                Ok(r) if r.status().is_success() => return r.text().await.map_err(|e| e.to_string()),
                Ok(r) if r.status().as_u16() == 404 => return Err(format!("{url}: HTTP 404")),
                Ok(r) => last = format!("HTTP {}", r.status()),
                Err(e) => last = e.to_string(),
            }
        }
        Err(format!("{url}: {last}"))
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn find_session(html: &str) -> Option<String> {
    let i = html.find("session = {\"id\":\"")? + "session = {\"id\":\"".len();
    let id = html.get(i..i + 32)?;
    id.bytes().all(|b| b.is_ascii_hexdigit()).then(|| id.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn session_parse() {
        let h = r#"var x; session = {"id":"21d6f40cfb511982e4424e0e250a9557","user_id":1}"#;
        assert_eq!(super::find_session(h).as_deref(), Some("21d6f40cfb511982e4424e0e250a9557"));
        assert_eq!(super::find_session("session = {\"id\":\"zz\"}"), None);
    }
}
