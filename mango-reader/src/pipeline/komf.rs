//! Durable, API-only metadata delivery. This module never writes canonical media.
use super::{Pipeline, VerificationPolicy};
use crate::{
    db::Db,
    error::{AppError, Result},
};
use reqwest::Client;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct KomfClient {
    http: Client,
    base: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Accepted {
    job_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    id: String,
    series_id: String,
    status: String,
}

impl KomfClient {
    pub fn new(_http: Client, url: &str) -> Result<Self> {
        let parsed =
            reqwest::Url::parse(url).map_err(|_| AppError::bad_request("Invalid KOMF URL"))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(AppError::bad_request(
                "KOMF URL must be HTTP(S) without credentials/query/fragment",
            ));
        }
        Ok(Self {
            http: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(|_| AppError::internal("Cannot build KOMF client"))?,
            base: url.trim_end_matches('/').into(),
        })
    }
    /// POST is never automatically retried. Only explicit HTTP rejection is known not to have created a job.
    pub async fn submit(
        &self,
        book: &str,
        library: &str,
        series: i64,
    ) -> std::result::Result<String, (&'static str, &'static str)> {
        let response = self.http.post(format!("{}/api/komga/metadata/identify/book", self.base))
            .json(&serde_json::json!({"bookId":book,"provider":"MANGA_BAKA","providerSeriesId":series.to_string(),"libraryId":library}))
            .send().await.map_err(|e| if e.is_connect() { ("unavailable","KOMF unavailable — metadata retry available") }
                else { ("unknown","KOMF submission outcome unknown; reconcile before resubmission") })?;
        if matches!(
            response.status().as_u16(),
            400 | 401 | 403 | 404 | 405 | 422
        ) {
            return Err((
                "rejected",
                "KOMF rejected metadata request — retry available",
            ));
        }
        if !response.status().is_success() {
            return Err((
                "unknown",
                "KOMF submission outcome unknown; reconcile before resubmission",
            ));
        }
        let accepted: Accepted = response.json().await.map_err(|_| {
            (
                "unknown",
                "KOMF accepted request but job ID is unknown; reconcile before resubmission",
            )
        })?;
        if !safe_id(&accepted.job_id) {
            return Err((
                "unknown",
                "KOMF returned an invalid job ID; reconcile before resubmission",
            ));
        }
        Ok(accepted.job_id)
    }
    pub async fn poll(
        &self,
        job: &str,
        series: &str,
        policy: VerificationPolicy,
    ) -> (&'static str, &'static str) {
        if !safe_id(job) {
            return ("unknown", "Invalid persisted KOMF job ID");
        }
        let start = Instant::now();
        let mut delay = policy.initial_delay;
        while start.elapsed() < policy.timeout {
            let remaining = policy.timeout.saturating_sub(start.elapsed());
            let result = tokio::time::timeout(remaining, async {
                let response = self
                    .http
                    .get(format!("{}/api/jobs/{job}", self.base))
                    .send()
                    .await
                    .ok()?;
                if !response.status().is_success() {
                    return None;
                }
                response.json::<Job>().await.ok()
            })
            .await
            .ok()
            .flatten();
            if let Some(value) = result {
                if value.id != job || value.series_id != series {
                    return (
                        "unknown",
                        "KOMF job identity mismatch; existing job retained",
                    );
                }
                match value.status.as_str() {
                    "COMPLETED" => {
                        return (
                            "succeeded",
                            "KOMF metadata job completed (artwork may be optional)",
                        )
                    }
                    "FAILED" => {
                        return (
                            "failed",
                            "KOMF metadata job failed — metadata-only retry available",
                        )
                    }
                    "RUNNING" => (),
                    _ => return ("unknown", "Unknown KOMF job status; existing job retained"),
                }
            }
            tokio::time::sleep(delay.min(policy.timeout.saturating_sub(start.elapsed()))).await;
            delay = Duration::from_secs_f64(
                (delay.as_secs_f64() * policy.backoff_factor)
                    .min(policy.max_interval.as_secs_f64()),
            );
        }
        (
            "unknown",
            "KOMF job timeout/unavailable — existing job retained for polling",
        )
    }
}
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Handoff {
    pub chapter_id: i64,
    pub canonical_path: String,
    pub state: String,
    pub komf_url: Option<String>,
    pub komga_book_id: Option<String>,
    pub komga_series_id: Option<String>,
    pub library_id: Option<String>,
    pub mangabaka_id: Option<i64>,
    pub job_id: Option<String>,
    pub message: String,
}
pub fn read(conn: &rusqlite::Connection, chapter: i64) -> rusqlite::Result<Option<Handoff>> {
    conn.query_row("SELECT chapter_id,canonical_path,state,komf_url,komga_book_id,komga_series_id,library_id,mangabaka_id,job_id,message FROM metadata_handoffs WHERE chapter_id=?1", [chapter], |r|Ok(Handoff {
        chapter_id:r.get(0)?,canonical_path:r.get(1)?,state:r.get(2)?,komf_url:r.get(3)?,komga_book_id:r.get(4)?,komga_series_id:r.get(5)?,library_id:r.get(6)?,mangabaka_id:r.get(7)?,job_id:r.get(8)?,message:r.get(9)?,
    })).optional()
}

async fn finish(db: &Db, chapter: i64, state: &str, message: &str) -> Result<bool> {
    let state = state.to_owned();
    let message = message.to_owned();
    db.call(move |conn| {
        if state != "succeeded" && read(conn,chapter)?.is_some_and(|h| h.state=="succeeded") { return Ok(true); }
        conn.execute("UPDATE metadata_handoffs SET state=?2,message=?3,updated_at=strftime('%s','now') WHERE chapter_id=?1", rusqlite::params![chapter,state,message])?;
        if state == "succeeded" { crate::acquisition::service::complete_queued_publication(conn, chapter)?; return Ok(true); }
        let queue_state = if state=="waiting_komga" {"verifying"} else if state=="needs_review" {"metadata_review"}
            else if state=="unknown" && read(conn,chapter)?.is_some_and(|h|h.job_id.is_some()) {"metadata_wait"} else {"metadata_failed"};
        conn.execute("UPDATE downloads SET state=?2,error=?3,retry_at=strftime('%s','now')+60 WHERE chapter_id=?1", rusqlite::params![chapter,queue_state,message])?;
        Ok(false)
    }).await
}

fn persisted_identity(
    conn: &rusqlite::Connection,
    chapter: i64,
    path: &str,
) -> Result<Option<i64>> {
    let mut q=conn.prepare("SELECT mangabaka_id, metadata_status FROM acquisition_history WHERE chapter_id=?1 AND canonical_path=?2")?;
    let rows = q.query_map(rusqlite::params![chapter, path], |r| {
        Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<String>>(1)?))
    })?;
    let mut ids = std::collections::BTreeSet::new();
    for row in rows {
        let (id, status) = row?;
        if status.is_some_and(|s| {
            let s = s.to_lowercase();
            s.contains("ambig") || s.contains("conflict") || s.contains("needs_review")
        }) {
            return Err(AppError::bad_request(
                "Ambiguous persisted MangaBaka identity",
            ));
        }
        if let Some(id) = id {
            if id <= 0 {
                return Err(AppError::bad_request("Invalid MangaBaka identity"));
            }
            ids.insert(id);
        }
    }
    if ids.len() > 1 {
        return Err(AppError::bad_request(
            "Conflicting publication MangaBaka identities",
        ));
    }
    Ok(ids.into_iter().next())
}

impl Pipeline {
    /// Called only after canonical publication (including recovery). Errors never invalidate that publication.
    pub async fn handoff_metadata(
        &self,
        chapter: i64,
        title: &str,
        path: &Path,
        library: Option<&str>,
        is_volume: bool,
        number: f64,
        retry: bool,
    ) -> Result<bool> {
        let (config, _, key) = self.load_internal_config().await?;
        if !config.komf_enabled {
            return Err(AppError::bad_request("KOMF handoff is disabled"));
        }
        let path = path.to_string_lossy().into_owned();
        let initial_path = path.clone();
        let existing = self.db.call(move |c| Ok(read(c, chapter)?)).await?;
        if let Some(ref old) = existing {
            if old.canonical_path != path {
                return Err(AppError::bad_request(
                    "Published path changed; reconcile existing handoff",
                ));
            }
            if old.state == "succeeded" {
                return finish(&self.db, chapter, "succeeded", &old.message).await;
            }
            if old.job_id.is_some() && !(old.state == "failed" && retry) {
                let client =
                    KomfClient::new(self.http.clone(), old.komf_url.as_deref().unwrap_or(""))?;
                let (state, message) = client
                    .poll(
                        old.job_id.as_deref().unwrap(),
                        old.komga_series_id.as_deref().unwrap_or(""),
                        self.verification_policy(),
                    )
                    .await;
                return finish(&self.db, chapter, state, message).await;
            }
            if old.state == "submitting" || (old.state == "unknown" && old.job_id.is_none()) {
                return finish(
                    &self.db,
                    chapter,
                    "unknown",
                    "KOMF submission outcome unknown; automatic resubmission blocked",
                )
                .await;
            }
            if matches!(
                old.state.as_str(),
                "failed" | "rejected" | "unavailable" | "needs_review"
            ) && !retry
            {
                return Ok(false);
            }
        }
        self.db.call(move|c| { c.execute("INSERT OR IGNORE INTO metadata_handoffs(chapter_id,canonical_path,state,message,updated_at) VALUES(?1,?2,'waiting_komga','Resolving exact Komga book',strftime('%s','now'))",rusqlite::params![chapter,initial_path])?; Ok(()) }).await?;
        let Some(library) = library.filter(|l| {
            !l.trim().is_empty()
                && Some(*l) == config.komga_library_id.as_deref()
                && Some(*l) != config.komga_chinese_library_id.as_deref()
        }) else {
            return finish(
                &self.db,
                chapter,
                "needs_review",
                "Missing/conflicting English Komga library; metadata needs review",
            )
            .await;
        };
        if !is_volume || !number.is_finite() {
            return finish(
                &self.db,
                chapter,
                "needs_review",
                "Chapter/unknown numbering is not supported by the automatic KOMF volume handoff",
            )
            .await;
        }
        let p = path.clone();
        let identity = match self
            .db
            .call(move |c| persisted_identity(c, chapter, &p))
            .await
        {
            Ok(i) => i,
            Err(_) => {
                return finish(
                    &self.db,
                    chapter,
                    "needs_review",
                    "Conflicting/ambiguous publication identity",
                )
                .await
            }
        };
        let known= self.db.call(move|c| {
            let mut q=c.prepare("SELECT komga_series_id FROM tracker_mappings WHERE tracker='mangabaka' AND confidence='resolved' AND tracker_id=?1")?;
            let rows=q.query_map([identity.map(|v|v.to_string()).unwrap_or_default()],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(if rows.len()==1 {rows.into_iter().next()} else {None})
        }).await?;
        // A previous exact resolution remains series identity, not a title guess.
        let known = existing
            .as_ref()
            .filter(|h| h.library_id.as_deref() == Some(library))
            .and_then(|h| h.komga_series_id.clone())
            .or(known);
        let komga = super::komga::KomgaClient::new(self.http.clone(), config.komga_url, key);
        let root = Path::new(config.library_dir.as_deref().unwrap_or(""));
        let start = Instant::now();
        let policy = self.verification_policy();
        let mut delay = policy.initial_delay;
        let book = loop {
            let result = tokio::time::timeout(
                policy.timeout.saturating_sub(start.elapsed()),
                komga.resolve_published_book(
                    title,
                    library,
                    Path::new(&path),
                    root,
                    known.as_deref(),
                    true,
                    number,
                ),
            )
            .await;
            match result {
                Ok(Ok(Some(b))) => break b,
                Ok(Err(e)) if e.status == 400 => {
                    return finish(
                        &self.db,
                        chapter,
                        "needs_review",
                        "Ambiguous/conflicting Komga book — metadata needs review",
                    )
                    .await
                }
                _ => (),
            }
            if start.elapsed() >= policy.timeout {
                return finish(
                    &self.db,
                    chapter,
                    "waiting_komga",
                    "Published — waiting for exact Komga book",
                )
                .await;
            }
            tokio::time::sleep(delay.min(policy.timeout.saturating_sub(start.elapsed()))).await;
            delay = Duration::from_secs_f64(
                (delay.as_secs_f64() * policy.backoff_factor)
                    .min(policy.max_interval.as_secs_f64()),
            );
        };
        let bound_book = book.id.clone();
        let bound_series = book.series_id.clone();
        let bound_library = library.to_owned();
        self.db.call(move|c| {
            c.execute("UPDATE metadata_handoffs SET komga_book_id=?2,komga_series_id=?3,library_id=?4 WHERE chapter_id=?1 AND job_id IS NULL AND state IN ('waiting_komga','needs_review')",rusqlite::params![chapter,bound_book,bound_series,bound_library])?;
            Ok(())
        }).await?;
        let manga_id = self
            .db
            .call(move |c| {
                Ok(c.query_row(
                    "SELECT manga_id FROM chapters WHERE id=?1",
                    [chapter],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .await?;
        self.ownership.invalidate(manga_id);
        let sid = book.series_id.clone();
        let mb=self.db.call(move|c| {
            let mapping:Option<(String,String)>=c.query_row("SELECT tracker_id,confidence FROM tracker_mappings WHERE komga_series_id=?1 AND tracker='mangabaka'",[sid],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            let mapped=if let Some((value,confidence))=mapping {
                if confidence!="resolved" {return Err(AppError::bad_request("Unresolved MangaBaka mapping"));}
                Some(value.parse::<i64>().ok().filter(|v|*v>0).ok_or_else(||AppError::bad_request("Invalid MangaBaka mapping"))?)
            } else {None};
            if identity.zip(mapped).is_some_and(|(a,b)|a!=b) {return Err(AppError::bad_request("Conflicting MangaBaka mappings"));}
            identity.or(mapped).ok_or_else(||AppError::bad_request("Missing authoritative MangaBaka identity"))
        }).await;
        let mb = match mb {
            Ok(v) => v,
            Err(_) => {
                return finish(
                    &self.db,
                    chapter,
                    "needs_review",
                    "Missing/ambiguous/conflicting MangaBaka identity — metadata needs review",
                )
                .await
            }
        };
        let client =
            match KomfClient::new(self.http.clone(), config.komf_url.as_deref().unwrap_or("")) {
                Ok(c) => c,
                Err(_) => {
                    return finish(
                        &self.db,
                        chapter,
                        "unavailable",
                        "Invalid KOMF configuration — metadata retry available",
                    )
                    .await
                }
            };
        let url = client.base.clone();
        let bid = book.id.clone();
        let sid = book.series_id.clone();
        let lib = library.to_owned();
        let claimed=self.db.call(move|c| {
            let current=read(c,chapter)?.ok_or_else(||AppError::internal("Missing handoff"))?;
            let allowed=matches!(current.state.as_str(),"waiting_komga") || (retry && matches!(current.state.as_str(),"failed"|"rejected"|"unavailable"|"needs_review"));
            if !allowed {return Ok(false);}
            Ok(c.execute("UPDATE metadata_handoffs SET state='submitting',komf_url=?2,komga_book_id=?3,komga_series_id=?4,library_id=?5,mangabaka_id=?6,job_id=NULL,message='Waiting for KOMF metadata',updated_at=strftime('%s','now') WHERE chapter_id=?1 AND state=?7",rusqlite::params![chapter,url,bid,sid,lib,mb,current.state])?==1)
        }).await?;
        if !claimed {
            return Ok(false);
        }
        match client.submit(&book.id, library, mb).await {
            Ok(job) => {
                let saved = job.clone();
                self.db.call(move|c| { c.execute("UPDATE metadata_handoffs SET job_id=?2,state='pending' WHERE chapter_id=?1",rusqlite::params![chapter,saved])?;Ok(()) }).await?;
                let (state, message) = client
                    .poll(&job, &book.series_id, self.verification_policy())
                    .await;
                finish(&self.db, chapter, state, message).await
            }
            Err((state, message)) => finish(&self.db, chapter, state, message).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::{Path as AxumPath, State},
        http::StatusCode,
        routing::{get, post},
        Json, Router,
    };
    use serde_json::{json, Value};
    use std::{
        fs,
        io::Write,
        path::PathBuf,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        },
    };
    #[derive(Clone)]
    struct Server {
        books: Arc<Mutex<Vec<Value>>>,
        posts: Arc<Mutex<Vec<Value>>>,
        job: Arc<Mutex<String>>,
        reject: Arc<AtomicUsize>,
        polls: Arc<AtomicUsize>,
        scans: Arc<AtomicUsize>,
        pending: Arc<AtomicUsize>,
    }
    struct Fixture {
        pipeline: Pipeline,
        db: Db,
        root: PathBuf,
        path: PathBuf,
        server: Server,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.task.abort();
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    fn cbz(path: &std::path::Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        zip.start_file("001.jpg", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"original page").unwrap();
        zip.finish().unwrap();
    }
    impl Fixture {
        async fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "mango-komf-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let library = root.join("library");
            let path = library.join("Handoff Series/Handoff Series v01.cbz");
            cbz(&path);
            let db = Db::open(&root.join("test.db")).unwrap();
            let server = Server {
                books: Arc::new(Mutex::new(vec![
                    json!({"id":"book-1","seriesId":"series-1","name":"Handoff Series v01.cbz","number":1,"url":"/remote/manga/Handoff Series/Handoff Series v01.cbz","metadata":{"number":"1"}}),
                ])),
                posts: Default::default(),
                job: Arc::new(Mutex::new("COMPLETED".into())),
                reject: Default::default(),
                polls: Default::default(),
                scans: Default::default(),
                pending: Default::default(),
            };
            let app=Router::new()
                .route("/api/v1/libraries",get(||async{Json(json!([{"id":"english","name":"Manga","root":"/remote/manga"}]))}))
                .route("/api/v1/libraries/english/scan",post(|State(s):State<Server>|async move{s.scans.fetch_add(1,Ordering::SeqCst);StatusCode::NO_CONTENT}))
                .route("/api/v1/series",get(||async{Json(json!({"content":[{"id":"series-1","libraryId":"english","name":"Handoff Series","booksCount":2}]}))}))
                .route("/api/v1/series/series-1/books",get(|State(s):State<Server>|async move{
                    let polls=s.polls.fetch_add(1,Ordering::SeqCst);let books=if polls<s.pending.load(Ordering::SeqCst){vec![]}else{s.books.lock().unwrap().clone()};Json(json!({"content":books}))
                }))
                .route("/api/komga/metadata/identify/book",post(|State(s):State<Server>,Json(body):Json<Value>|async move{
                    s.posts.lock().unwrap().push(body);let status=s.reject.load(Ordering::SeqCst);
                    if status>0 {(StatusCode::from_u16(status as u16).unwrap(),Json(json!({})))}else{(StatusCode::OK,Json(json!({"jobId":"job-1"})))}
                }))
                .route("/api/jobs/{job}",get(|State(s):State<Server>,AxumPath(id):AxumPath<String>|async move{Json(json!({"id":id,"seriesId":"series-1","status":s.job.lock().unwrap().clone()}))}))
                .with_state(server.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let task = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let lib = library.to_string_lossy().to_string();
            let dest = path.to_string_lossy().to_string();
            db.call(move|c|{
                c.execute("INSERT INTO manga(id,source_id,url,title) VALUES(1,0,'/s','Handoff Series')",[])?;
                c.execute("INSERT INTO chapters(id,manga_id,url,name,chapter_number,source_order,fetched_at) VALUES(1,1,'/v1','Volume 1',1,1,0)",[])?;
                c.execute("INSERT INTO downloads(chapter_id,position,state,added_at,published_path,target_library_id) VALUES(1,1,'verifying',0,?1,'english')",[&dest])?;
                c.execute("INSERT INTO acquisition_history(chapter_id,manga_id,series,release_name,outcome,canonical_path,completed_at,mangabaka_id) VALUES(1,1,'Handoff Series','Volume 1','published',?1,0,83)",[dest])?;
                for(key,value) in [("pipeline.enabled",json!(true)),("pipeline.library_dir",json!(lib)),("pipeline.komga_enabled",json!(true)),("pipeline.komga_library_id",json!("english")),("pipeline.komga_url",json!(format!("http://{addr}"))),("pipeline.komf_enabled",json!(true)),("pipeline.komf_url",json!(format!("http://{addr}"))),("pipeline.mangabaka_enabled",json!(false)),("pipeline.optimize_enabled",json!(false))] {crate::settings::set(c,key,&value)?;} Ok(())
            }).await.unwrap();
            let pipeline = Pipeline::new(db.clone());
            pipeline.set_verification_policy(VerificationPolicy::fast_test());
            Self {
                pipeline,
                db,
                root,
                path,
                server,
                task,
            }
        }
        async fn run(&self, retry: bool) -> bool {
            self.pipeline
                .handoff_metadata(
                    1,
                    "Handoff Series",
                    &self.path,
                    Some("english"),
                    true,
                    1.0,
                    retry,
                )
                .await
                .unwrap()
        }
        async fn record(&self) -> Handoff {
            self.db.call(|c| Ok(read(c, 1)?.unwrap())).await.unwrap()
        }
        fn posts(&self) -> usize {
            self.server.posts.lock().unwrap().len()
        }
        async fn mapping(&self, id: &str, confidence: &str) {
            let id = id.to_owned();
            let confidence = confidence.to_owned();
            self.db.call(move|c|{c.execute("INSERT OR REPLACE INTO tracker_mappings(komga_series_id,tracker,tracker_id,updated_at,confidence) VALUES('series-1','mangabaka',?1,0,?2)",rusqlite::params![id,confidence])?;Ok(())}).await.unwrap();
        }
    }
    #[tokio::test]
    async fn komf_happy_path_payload_exact_isolation_and_restart_success() {
        let f = Fixture::new().await;
        let before = fs::read(&f.path).unwrap();
        f.server.pending.store(2, Ordering::SeqCst);
        assert!(f.run(false).await);
        assert!(f.server.polls.load(Ordering::SeqCst) >= 3);
        assert_eq!(f.posts(), 1);
        assert_eq!(
            f.server.posts.lock().unwrap()[0],
            json!({"bookId":"book-1","provider":"MANGA_BAKA","providerSeriesId":"83","libraryId":"english"})
        );
        assert_eq!(f.record().await.state, "succeeded");
        assert!(f.run(false).await);
        assert_eq!(f.posts(), 1);
        assert_eq!(before, fs::read(&f.path).unwrap());
    }
    #[tokio::test]
    async fn komf_missing_and_duplicate_books_never_submit() {
        for duplicate in [false, true] {
            let f = Fixture::new().await;
            {
                let mut b = f.server.books.lock().unwrap();
                if duplicate {
                    let mut other = b[0].clone();
                    other["id"] = json!("book-2");
                    b.push(other);
                } else {
                    b.clear();
                }
            }
            assert!(!f.run(false).await);
            assert_eq!(f.posts(), 0);
            assert_eq!(
                f.record().await.state,
                if duplicate {
                    "needs_review"
                } else {
                    "waiting_komga"
                }
            );
        }
    }
    #[tokio::test]
    async fn komf_identity_missing_conflicting_and_ambiguous_fail_closed() {
        for case in 0..4 {
            let f = Fixture::new().await;
            match case {
                0 => {
                    f.db.call(|c| {
                        c.execute("UPDATE acquisition_history SET mangabaka_id=NULL", [])?;
                        Ok(())
                    })
                    .await
                    .unwrap();
                }
                1 => f.mapping("84", "resolved").await,
                2 => f.mapping("83", "ambiguous").await,
                _ => {
                    f.db.call(|c| {
                        c.execute(
                            "UPDATE acquisition_history SET metadata_status='ambiguous'",
                            [],
                        )?;
                        Ok(())
                    })
                    .await
                    .unwrap();
                }
            }
            assert!(!f.run(false).await);
            assert_eq!(f.posts(), 0);
            assert_eq!(f.record().await.state, "needs_review");
        }
    }
    #[tokio::test]
    async fn komf_resolved_mapping_can_supply_identity_without_title_search() {
        let f = Fixture::new().await;
        f.db.call(|c| {
            c.execute("UPDATE acquisition_history SET mangabaka_id=NULL", [])?;
            Ok(())
        })
        .await
        .unwrap();
        f.mapping("83", "resolved").await;
        assert!(f.run(false).await);
        assert_eq!(f.record().await.mangabaka_id, Some(83));
    }
    #[tokio::test]
    async fn komf_terminal_failure_and_metadata_only_retry_preserve_archive() {
        let f = Fixture::new().await;
        let before = fs::read(&f.path).unwrap();
        *f.server.job.lock().unwrap() = "FAILED".into();
        assert!(!f.run(false).await);
        assert_eq!(f.record().await.state, "failed");
        assert_eq!(f.posts(), 1);
        *f.server.job.lock().unwrap() = "COMPLETED".into();
        assert!(f.run(true).await);
        assert_eq!(f.posts(), 2);
        assert_eq!(before, fs::read(&f.path).unwrap());
        assert_eq!(f.server.scans.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn komf_unknown_retains_job_and_resumes_polling_without_submission() {
        let f = Fixture::new().await;
        *f.server.job.lock().unwrap() = "RUNNING".into();
        let before = fs::read(&f.path).unwrap();
        assert!(!f.run(false).await);
        assert_eq!(f.record().await.job_id.as_deref(), Some("job-1"));
        let restarted = Pipeline::new(f.db.clone());
        restarted.set_verification_policy(VerificationPolicy::fast_test());
        *f.server.job.lock().unwrap() = "COMPLETED".into();
        assert!(restarted
            .handoff_metadata(
                1,
                "Handoff Series",
                &f.path,
                Some("english"),
                true,
                1.0,
                false
            )
            .await
            .unwrap());
        assert_eq!(f.posts(), 1);
        assert_eq!(before, fs::read(&f.path).unwrap());
        assert_eq!(f.server.scans.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn komf_rejected_and_uncertain_submission_have_distinct_retry_safety() {
        for code in [400, 408, 500, 200, 302] {
            let f = Fixture::new().await;
            f.server.reject.store(code, Ordering::SeqCst);
            let before = fs::read(&f.path).unwrap();
            assert!(!f.run(false).await);
            f.server.reject.store(0, Ordering::SeqCst);
            assert_eq!(f.run(true).await, code == 400);
            assert_eq!(f.posts(), if code == 400 { 2 } else { 1 });
            assert_eq!(before, fs::read(&f.path).unwrap());
        }
    }
    #[tokio::test]
    async fn komf_crash_in_submission_never_reposts() {
        let f = Fixture::new().await;
        let path = f.path.to_string_lossy().to_string();
        f.db.call(move|c|{c.execute("INSERT INTO metadata_handoffs(chapter_id,canonical_path,state,message,updated_at) VALUES(1,?1,'submitting','in flight',0)",[path])?;Ok(())}).await.unwrap();
        assert!(!f.run(true).await);
        assert_eq!(f.posts(), 0);
        assert_eq!(f.record().await.state, "unknown");
    }
    #[tokio::test]
    async fn komf_library_mismatch_chapters_and_number_conflict_never_mutate() {
        let f = Fixture::new().await;
        assert!(!f
            .pipeline
            .handoff_metadata(
                1,
                "Handoff Series",
                &f.path,
                Some("other"),
                true,
                1.0,
                false
            )
            .await
            .unwrap());
        assert_eq!(f.posts(), 0);
        let f = Fixture::new().await;
        assert!(!f
            .pipeline
            .handoff_metadata(
                1,
                "Handoff Series",
                &f.path,
                Some("english"),
                false,
                1.0,
                false
            )
            .await
            .unwrap());
        assert_eq!(f.posts(), 0);
        let f = Fixture::new().await;
        f.server.books.lock().unwrap()[0]["metadata"]["number"] = json!("2");
        assert!(!f.run(false).await);
        assert_eq!(f.posts(), 0);
    }
    #[tokio::test]
    async fn komf_path_mapping_nested_names_and_escape_rejection() {
        let f = Fixture::new().await;
        let client = super::super::komga::KomgaClient::new(
            f.pipeline.http.clone(),
            f.pipeline.load_config().await.unwrap().komga_url,
            None,
        );
        let path = f
            .root
            .join("library/Publisher (EN)/Nested & Series/v01 [Official!].cbz");
        cbz(&path);
        f.server.books.lock().unwrap()[0]["url"] =
            json!("/remote/manga/Publisher (EN)/Nested & Series/v01 [Official!].cbz");
        assert!(client
            .resolve_published_book(
                "Handoff Series",
                "english",
                &path,
                &f.root.join("library"),
                None,
                true,
                1.0
            )
            .await
            .unwrap()
            .is_some());
        f.server.books.lock().unwrap()[0]["url"] = json!("/remote/manga/../escape.cbz");
        assert!(client
            .resolve_published_book(
                "Handoff Series",
                "english",
                &path,
                &f.root.join("library"),
                None,
                true,
                1.0
            )
            .await
            .unwrap()
            .is_none());
        assert_eq!(f.posts(), 0);
    }
    #[tokio::test]
    async fn komf_disabled_keeps_existing_publication_behavior() {
        let f = Fixture::new().await;
        f.db.call(|c| {
            crate::settings::set(c, "pipeline.komf_enabled", &json!(false))?;
            Ok(())
        })
        .await
        .unwrap();
        assert!(!f.pipeline.load_config().await.unwrap().komf_enabled);
        assert!(f
            .pipeline
            .handoff_metadata(
                1,
                "Handoff Series",
                &f.path,
                Some("english"),
                true,
                1.0,
                false
            )
            .await
            .is_err());
        assert_eq!(f.posts(), 0);
    }
    #[tokio::test]
    async fn komf_full_publication_scans_then_exact_job_completes() {
        let f = Fixture::new().await;
        fs::remove_file(&f.path).unwrap();
        let staged = f.root.join("staged.cbz");
        cbz(&staged);
        let job = crate::downloads::Job {
            chapter: 1,
            manga_id: 1,
            title: "Handoff Series".into(),
            name: "Volume 1".into(),
            number: 1.0,
            volume: Some(1.0),
            release_type: crate::release::ReleaseType::Volume,
            lang: Some("en".into()),
            ..Default::default()
        };
        let result = f.pipeline.process_download(&staged, &job).await.unwrap();
        assert!(matches!(
            result,
            super::super::AcquisitionOutcome::Complete(_)
        ));
        assert_eq!(f.server.scans.load(Ordering::SeqCst), 1);
        assert_eq!(f.posts(), 1);
        assert_eq!(f.record().await.state, "succeeded");
        assert!(f.path.is_file());
    }
    #[tokio::test]
    async fn komf_downloads_metadata_only_retry_never_reprocesses_or_scans() {
        let f = Fixture::new().await;
        *f.server.job.lock().unwrap() = "FAILED".into();
        let before = fs::read(&f.path).unwrap();
        assert!(!f.run(false).await);
        let host = crate::host::Host::new(
            crate::config::HostConfig::default(),
            f.root.join("host"),
            &f.root,
        )
        .unwrap();
        let images = crate::images::Images::new(
            f.db.clone(),
            host,
            crate::local::Local::new(f.root.join("local")),
            &f.root,
            10,
        );
        let mut downloads = crate::downloads::Downloads::new(f.db.clone(), images, &f.root, 0);
        downloads.set_pipeline(Arc::new(f.pipeline.clone()));
        *f.server.job.lock().unwrap() = "COMPLETED".into();
        assert!(downloads
            .retry_metadata(1)
            .await
            .unwrap()
            .contains("completed"));
        assert_eq!(before, fs::read(&f.path).unwrap());
        assert_eq!(f.posts(), 2);
        assert_eq!(f.server.scans.load(Ordering::SeqCst), 0);
        assert!(downloads
            .retry_metadata(1)
            .await
            .unwrap()
            .contains("completed"));
        assert_eq!(f.posts(), 2);
    }
    #[tokio::test]
    async fn komf_concurrent_handoffs_submit_at_most_once() {
        let f = Fixture::new().await;
        let _ = tokio::join!(f.run(false), f.run(false));
        assert_eq!(f.posts(), 1);
        assert_eq!(f.record().await.state, "succeeded");
    }
    #[tokio::test]
    async fn komf_url_configuration_rejects_credentials_and_non_http() {
        for url in [
            "",
            "file:///tmp/a",
            "http://user:pass@example.test",
            "https://example.test?token=secret",
        ] {
            assert!(KomfClient::new(Client::new(), url).is_err());
        }
        assert!(KomfClient::new(Client::new(), "https://komf.example/prefix").is_ok());
        let f = Fixture::new().await;
        let mut config = f.pipeline.load_config().await.unwrap();
        config.komf_enabled = false;
        config.komf_url = Some("http://user:pass@example.test".into());
        assert!(f.pipeline.save_config(config).await.is_err());
    }
    #[tokio::test]
    async fn komf_unavailable_preserves_published_book_and_allows_explicit_retry() {
        let f = Fixture::new().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        f.db.call(move |c| {
            crate::settings::set(c, "pipeline.komf_url", &json!(format!("http://{addr}")))?;
            Ok(())
        })
        .await
        .unwrap();
        let before = fs::read(&f.path).unwrap();
        assert!(!f.run(false).await);
        assert_eq!(f.record().await.state, "unavailable");
        assert_eq!(f.posts(), 0);
        assert_eq!(before, fs::read(&f.path).unwrap());
    }
    #[tokio::test]
    async fn komf_actual_download_worker_restart_resumes_known_job_without_scan_or_source() {
        let f = Fixture::new().await;
        *f.server.job.lock().unwrap() = "RUNNING".into();
        assert!(!f.run(false).await);
        let before = fs::read(&f.path).unwrap();
        let book_polls = f.server.polls.load(Ordering::SeqCst);
        f.server.books.lock().unwrap().clear();
        *f.server.job.lock().unwrap() = "COMPLETED".into();
        f.db.call(|c| {
            c.execute("UPDATE downloads SET retry_at=0", [])?;
            Ok(())
        })
        .await
        .unwrap();
        let host = crate::host::Host::new(
            crate::config::HostConfig::default(),
            f.root.join("host-worker"),
            &f.root,
        )
        .unwrap();
        let images = crate::images::Images::new(
            f.db.clone(),
            host,
            crate::local::Local::new(f.root.join("local-worker")),
            &f.root,
            10,
        );
        let mut downloads = crate::downloads::Downloads::new(f.db.clone(), images, &f.root, 0);
        let restarted = Pipeline::new(f.db.clone());
        restarted.set_verification_policy(VerificationPolicy::fast_test());
        downloads.set_pipeline(Arc::new(restarted));
        let worker = tokio::spawn(downloads.run());
        tokio::time::timeout(Duration::from_secs(6), async {
            loop {
                let remaining =
                    f.db.call(|c| {
                        Ok(c.query_row(
                            "SELECT count(*) FROM downloads WHERE chapter_id=1",
                            [],
                            |r| r.get::<_, i64>(0),
                        )?)
                    })
                    .await
                    .unwrap();
                if remaining == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        worker.abort();
        assert_eq!(f.posts(), 1);
        assert_eq!(f.server.scans.load(Ordering::SeqCst), 0);
        assert_eq!(f.server.polls.load(Ordering::SeqCst), book_polls);
        assert_eq!(before, fs::read(&f.path).unwrap());
        assert_eq!(f.record().await.state, "succeeded");
    }
}
