//! AniList: one GraphQL address. Signing in is a token the owner gets there and pastes in,
//! since AniList only sends tokens to addresses registered with it.

use super::{
    Account, Entry, Found, Kind, Track, ask, judge, label, plain, said, sign_in, signed_out, text,
    tracker_error, year,
};
use crate::error::{AppError, Result};
use reqwest::Client;
use serde_json::{Value, json};

const API: &str = "https://graphql.anilist.co";
const STATUSES: [(i32, &str); 6] = [
    (1, "CURRENT"),
    (2, "COMPLETED"),
    (3, "PAUSED"),
    (4, "DROPPED"),
    (5, "PLANNING"),
    (6, "REPEATING"),
];

const SEARCH: &str = "query ($q: String) { Page(perPage: 20) { media(search: $q, type: MANGA, format_not_in: [NOVEL]) {
    id title { userPreferred english romaji native } format chapters volumes description startDate { year } siteUrl } } }";
// Scores are asked for out of 100 whatever scale the account shows them in.
const FIND: &str = "query ($user: Int, $media: Int) { Page { mediaList(userId: $user, mediaId: $media, type: MANGA) {
    id status progress progressVolumes score(format: POINT_100) media { chapters volumes } } } }";
const SAVE: &str = "mutation ($media: Int, $status: MediaListStatus, $progress: Int, $progressVolumes: Int, $score: Int) {
    SaveMediaListEntry(mediaId: $media, status: $status, progress: $progress, progressVolumes: $progressVolumes, scoreRaw: $score) { id } }";
const USER_LIST: &str = "query ($userId: Int, $page: Int) { Page(page: $page, perPage: 50) { pageInfo { total hasNextPage } mediaList(userId: $userId, type: MANGA, sort: UPDATED_TIME_DESC) { id mediaId status progress progressVolumes score(format: POINT_100) updatedAt media { id title { userPreferred english romaji native } coverImage { extraLarge large medium } format status chapters volumes siteUrl } } } }";

fn request(
    http: &Client,
    token: Option<&str>,
    query: &str,
    variables: Value,
) -> reqwest::RequestBuilder {
    let request = http
        .post(API)
        .json(&json!({ "query": query, "variables": variables }));
    match token {
        Some(token) => request.bearer_auth(token),
        None => request,
    }
}

async fn graphql(
    http: &Client,
    token: Option<&str>,
    query: &str,
    variables: Value,
) -> Result<Value> {
    let (status, body) = ask(Kind::AniList, request(http, token, query, variables)).await?;
    // AniList calls a token that ran out a bad request, not an unauthorized one.
    if token.is_some() && said(&body) == "Invalid token" {
        return Err(signed_out(Kind::AniList));
    }
    let mut body = judge(Kind::AniList, status, body)?;
    Ok(body.get_mut("data").map(Value::take).unwrap_or_default())
}

pub(super) async fn login(http: &Client, pasted: &str) -> Result<Account> {
    // The whole address of the page AniList shows will do too: the token is in it.
    let pasted = pasted.trim();
    let token = pasted
        .split_once("access_token=")
        .map_or(pasted, |(_, rest)| rest.split('&').next().unwrap_or(rest));
    if token.is_empty() {
        return Err(AppError::bad_request("Paste the token AniList shows"));
    }
    let body = sign_in(
        Kind::AniList,
        request(http, Some(token), "query { Viewer { id name } }", json!({})),
    )
    .await?;
    let viewer = &body["data"]["Viewer"];
    let id = viewer["id"]
        .as_i64()
        .ok_or_else(|| tracker_error("AniList did not say whose token this is".into()))?;
    Ok(Account {
        token: token.to_owned(),
        user_id: id.to_string(),
        user_name: text(&viewer["name"]),
        ..Account::default()
    })
}

/// Open to everyone, so it works before signing in and with a token that ran out.
pub(crate) async fn search(http: &Client, query: &str) -> Result<Vec<Found>> {
    let data = graphql(http, None, SEARCH, json!({ "q": query })).await?;
    let found = data["Page"]["media"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|media| {
            let media_id = media["id"].as_i64()?;
            Some(Found {
                media_id,
                title: text(&media["title"]["userPreferred"]),
                summary: plain(&media["description"]),
                kind: label(&media["format"]),
                year: year(&media["startDate"]["year"]),
                total_chapters: media["chapters"].as_i64().unwrap_or(0),
                url: format!("https://anilist.co/manga/{media_id}"),
            })
        });
    Ok(found.collect())
}

pub(crate) async fn find(http: &Client, account: &Account, media: i64) -> Result<Option<Entry>> {
    let user: i64 = account
        .user_id
        .parse()
        .map_err(|_| signed_out(Kind::AniList))?;
    let data = graphql(
        http,
        Some(&account.token),
        FIND,
        json!({ "user": user, "media": media }),
    )
    .await?;
    let Some(entry) = data["Page"]["mediaList"].get(0) else {
        return Ok(None);
    };
    Ok(Some(Entry {
        library_id: entry["id"].as_i64().unwrap_or(0),
        status: STATUSES
            .iter()
            .find(|(_, name)| entry["status"].as_str() == Some(*name))
            .map_or(1, |(number, _)| *number),
        last_chapter_read: entry["progress"].as_f64().unwrap_or(0.0),
        last_volume_read: entry["progressVolumes"].as_f64().unwrap_or(0.0),
        score: entry["score"].as_f64().unwrap_or(0.0),
        total_chapters: entry["media"]["chapters"].as_i64().unwrap_or(0),
        total_volumes: entry["media"]["volumes"].as_i64().unwrap_or(0),
    }))
}

pub(crate) async fn save_progress(
    http: &Client,
    account: &Account,
    media_id: i64,
    status: i32,
    score: f64,
    progress_chapters: Option<i64>,
    progress_volumes: Option<i64>,
) -> Result<i64> {
    let status_str = STATUSES
        .iter()
        .find(|(number, _)| *number == status)
        .map_or("CURRENT", |(_, name)| *name);
    let mut variables = json!({
        "media": media_id,
        "status": status_str,
        "score": score as i64,
    });
    if let Some(ch) = progress_chapters {
        variables["progress"] = json!(ch);
    }
    if let Some(vol) = progress_volumes {
        variables["progressVolumes"] = json!(vol);
    }
    let data = graphql(http, Some(&account.token), SAVE, variables).await?;
    data["SaveMediaListEntry"]["id"]
        .as_i64()
        .ok_or_else(|| tracker_error("AniList did not confirm the change".into()))
}

pub(crate) async fn save(http: &Client, account: &Account, track: &Track) -> Result<i64> {
    save_progress(
        http,
        account,
        track.media_id,
        track.status,
        track.score,
        Some(track.last_chapter_read as i64),
        None,
    )
    .await
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct AniListMangaListEntry {
    pub id: i64,
    pub media_id: i64,
    pub title: String,
    pub cover_url: Option<String>,
    pub status: String, // "CURRENT", "PLANNING", "COMPLETED", "PAUSED", "DROPPED", "REPEATING"
    pub progress_chapter: f64,
    pub progress_volume: f64,
    pub total_chapters: Option<i64>,
    pub total_volumes: Option<i64>,
    pub format: Option<String>,
    pub media_status: Option<String>,
    pub updated_at: i64,
}

pub(crate) async fn fetch_user_manga_list(
    http: &Client,
    account: &Account,
) -> Result<Vec<AniListMangaListEntry>> {
    let user: i64 = account
        .user_id
        .parse()
        .map_err(|_| signed_out(Kind::AniList))?;

    let mut entries = Vec::new();
    let mut page = 1;

    loop {
        let data = graphql(
            http,
            Some(&account.token),
            USER_LIST,
            json!({ "userId": user, "page": page }),
        )
        .await?;

        let page_node = &data["Page"];
        let list = page_node["mediaList"].as_array();
        let Some(items) = list else {
            break;
        };
        if items.is_empty() {
            break;
        }

        for item in items {
            let id = item["id"].as_i64().unwrap_or(0);
            let media_id = item["mediaId"].as_i64().unwrap_or(0);
            if media_id == 0 {
                continue;
            }
            let status = text(&item["status"]);
            let progress = item["progress"].as_f64().unwrap_or(0.0);
            let progress_volumes = item["progressVolumes"].as_f64().unwrap_or(0.0);
            let updated_at = item["updatedAt"]
                .as_i64()
                .unwrap_or_else(|| crate::db::now());

            let media = &item["media"];
            let mut title = text(&media["title"]["userPreferred"]);
            if title.is_empty() {
                title = text(&media["title"]["english"]);
            }
            if title.is_empty() {
                title = text(&media["title"]["romaji"]);
            }
            if title.is_empty() {
                title = format!("AniList Media {}", media_id);
            }

            let cover_url = media["coverImage"]["extraLarge"]
                .as_str()
                .or_else(|| media["coverImage"]["large"].as_str())
                .or_else(|| media["coverImage"]["medium"].as_str())
                .map(|s| s.to_string());

            let format = label(&media["format"]);
            let format_str = if format.is_empty() {
                None
            } else {
                Some(format)
            };
            let media_status = media["status"].as_str().map(|s| s.to_string());
            let total_chapters = media["chapters"].as_i64();
            let total_volumes = media["volumes"].as_i64();

            entries.push(AniListMangaListEntry {
                id,
                media_id,
                title,
                cover_url,
                status,
                progress_chapter: progress,
                progress_volume: progress_volumes,
                total_chapters,
                total_volumes,
                format: format_str,
                media_status,
                updated_at,
            });
        }

        let has_next = page_node["pageInfo"]["hasNextPage"]
            .as_bool()
            .unwrap_or(false);
        if !has_next {
            break;
        }
        page += 1;
        if page > 100 {
            break;
        }
    }

    Ok(entries)
}
