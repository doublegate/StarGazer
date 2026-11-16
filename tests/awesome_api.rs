//! GitHub client features used by the Awesome browser, against a mock server.

use serde_json::json;
use stargazer::awesome::{self, Store};
use stargazer::github::{FetchEvent, GitHubClient, GitHubError, META_BATCH};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

fn noop() -> impl FnMut(FetchEvent) + Send {
    |_| {}
}

#[tokio::test]
async fn readme_text_conditional_and_capped() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/sindresorhus/awesome/readme"))
        .and(header("if-none-match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/sindresorhus/awesome/readme"))
        .and(header("accept", "application/vnd.github.raw+json"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"v1\"")
                .set_body_string("# Awesome\n\n## Platforms\n"),
        )
        .mount(&server)
        .await;

    let client = GitHubClient::new(server.uri(), None).unwrap();
    let fresh = client
        .fetch_readme_text("sindresorhus/awesome", None, 1 << 20)
        .await
        .unwrap();
    assert_eq!(fresh.text.as_deref(), Some("# Awesome\n\n## Platforms\n"));
    assert_eq!(fresh.etag.as_deref(), Some("\"v1\""));

    let same = client
        .fetch_readme_text("sindresorhus/awesome", Some("\"v1\""), 1 << 20)
        .await
        .unwrap();
    assert_eq!(same.text, None, "304 means unchanged");
    assert_eq!(same.etag.as_deref(), Some("\"v1\""));

    let capped = client
        .fetch_readme_text("sindresorhus/awesome", None, 9)
        .await
        .unwrap();
    assert_eq!(capped.text.as_deref(), Some("# Awesome"));

    assert!(matches!(
        client.fetch_readme_text("o/../x", None, 100).await,
        Err(GitHubError::InvalidRepo(_))
    ));
}

/// Answers a metadata query with one repository per alias in the request,
/// echoing the requested owner/name; repo names starting with "gone" are
/// reported missing.
struct EchoMeta;

impl Respond for EchoMeta {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        let vars = body["variables"].as_object().unwrap();
        let mut data = serde_json::Map::new();
        let mut errors = Vec::new();
        for k in 0..vars.len() / 2 {
            let owner = vars[&format!("o{k}")].as_str().unwrap();
            let name = vars[&format!("n{k}")].as_str().unwrap();
            if name.starts_with("gone") {
                data.insert(format!("r{k}"), serde_json::Value::Null);
                errors.push(
                    json!({"type": "NOT_FOUND", "path": [format!("r{k}")], "message": "nope"}),
                );
                continue;
            }
            data.insert(
                format!("r{k}"),
                json!({
                    "nameWithOwner": format!("{owner}/{name}"), "databaseId": k + 1,
                    "stargazerCount": 10 * (k + 1), "description": "d",
                    "url": format!("https://github.com/{owner}/{name}"),
                    "primaryLanguage": {"name": "Rust"},
                    "repositoryTopics": {"nodes": []},
                    "pushedAt": "2026-11-01T00:00:00Z", "updatedAt": "2026-11-01T00:00:00Z",
                    "isArchived": false, "isFork": false
                }),
            );
        }
        ResponseTemplate::new(200).set_body_json(json!({"data": data, "errors": errors}))
    }
}

#[tokio::test]
async fn repos_meta_batches_keeps_order_and_skips_bad_names() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(header("authorization", "Bearer tk"))
        .respond_with(EchoMeta)
        .expect(2) // 150 names -> two batches of META_BATCH
        .mount(&server)
        .await;
    let client = GitHubClient::new(server.uri(), Some("tk".into())).unwrap();

    let mut names: Vec<String> = (0..148).map(|i| format!("o/r{i}")).collect();
    names.insert(5, "o/gone-repo".into());
    names.insert(9, "bad/../name".into()); // invalid: must never be sent
    assert_eq!(names.len(), 150);
    assert!(names.len() > META_BATCH);

    let mut events = Vec::new();
    let mut progress = |e: FetchEvent| events.push(e);
    let fetch = client
        .fetch_repos_meta(&names, &mut progress)
        .await
        .unwrap();
    assert!(fetch.error.is_none());
    let meta = fetch.metas;

    assert_eq!(meta.len(), 150, "one slot per input, in order");
    assert_eq!(meta[0].as_ref().unwrap().name_with_owner, "o/r0");
    assert_eq!(meta[5], None, "missing repo");
    assert_eq!(meta[9], None, "invalid name");
    assert_eq!(meta[149].as_ref().unwrap().name_with_owner, "o/r147");
    assert!(meta.iter().filter(|m| m.is_some()).count() == 148);
    assert!(events.len() >= 2, "progress per batch");
}

#[tokio::test]
async fn repos_meta_never_sends_invalid_names() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("evil"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let client = GitHubClient::new(server.uri(), Some("tk".into())).unwrap();
    let names = vec!["o/x\"}evil".to_string(), "../evil".to_string()];
    let fetch = client.fetch_repos_meta(&names, &mut noop()).await.unwrap();
    assert_eq!(fetch.metas, vec![None, None]);
}

const INDEX_MD: &str = include_str!("fixtures/awesome/index.md");
const LIST_MD: &str = include_str!("fixtures/awesome/list.md");

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::beside(&dir.path().join("stars.json"));
    (dir, store)
}

#[tokio::test]
async fn index_sync_parses_caches_and_revalidates() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/sindresorhus/awesome/readme"))
        .and(header("if-none-match", "\"i1\""))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/sindresorhus/awesome/readme"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"i1\"")
                .set_body_string(INDEX_MD),
        )
        .mount(&server)
        .await;
    let client = GitHubClient::new(server.uri(), None).unwrap();
    let (_dir, store) = store();

    let first = awesome::sync_index(&client, &store, false).await.unwrap();
    assert_eq!(first.index.sections.len(), 2);
    assert_eq!(store.load_index().unwrap().index, first.index, "persisted");

    // Second sync sends the ETag, gets 304, keeps the parsed index.
    let second = awesome::sync_index(&client, &store, false).await.unwrap();
    assert_eq!(second.index, first.index);
}

#[tokio::test]
async fn index_sync_failure_or_empty_index_keeps_the_cache() {
    let server = MockServer::start().await;
    let client = GitHubClient::new(server.uri(), None).unwrap();
    let (_dir, store) = store();
    Mock::given(method("GET"))
        .and(path("/repos/sindresorhus/awesome/readme"))
        .respond_with(ResponseTemplate::new(200).set_body_string(INDEX_MD))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let good = awesome::sync_index(&client, &store, true).await.unwrap();

    // Now the README "breaks": no sections at all. Must not replace the cache.
    Mock::given(method("GET"))
        .and(path("/repos/sindresorhus/awesome/readme"))
        .respond_with(ResponseTemplate::new(200).set_body_string("# Awesome\n\nMoved.\n"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    assert!(matches!(
        awesome::sync_index(&client, &store, true).await,
        Err(GitHubError::Parse(_))
    ));
    assert_eq!(store.load_index().unwrap().index, good.index);

    // And a server error leaves it untouched too.
    Mock::given(method("GET"))
        .and(path("/repos/sindresorhus/awesome/readme"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    assert!(awesome::sync_index(&client, &store, true).await.is_err());
    assert_eq!(store.load_index().unwrap().index, good.index);
}

#[tokio::test]
async fn list_fetch_with_token_maps_metadata_and_dedupes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/o/awesome-example/readme"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"l1\"")
                .set_body_string(format!(
                    "{LIST_MD}\n## Again\n\n- [ad-si/Woxi](https://github.com/ad-si/Woxi) - duplicate\n- [gone](https://github.com/o/gone-repo) - deleted\n"
                )),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(EchoMeta)
        .expect(1) // 6 distinct repos: one batch, the duplicate asked once
        .mount(&server)
        .await;
    let client = GitHubClient::new(server.uri(), Some("tk".into())).unwrap();
    let (_dir, store) = store();

    let list = awesome::fetch_list(
        &client,
        &store,
        "o/awesome-example",
        false,
        &HashMap::new(),
        &mut noop(),
    )
    .await
    .unwrap();
    assert!(list.meta_complete);
    assert_eq!(
        store.load_list("o/awesome-example").unwrap(),
        list,
        "persisted"
    );

    let rows = awesome::to_rows(&list);
    let woxi: Vec<_> = rows
        .iter()
        .filter(|r| r.full_name == "ad-si/Woxi")
        .collect();
    assert_eq!(woxi.len(), 1, "duplicate entry shown once");
    assert!(!woxi[0].external);
    assert!(woxi[0].stargazers_count > 0);
    assert_eq!(woxi[0].category, "Applications › Interpreters");

    let gone = rows.iter().find(|r| r.name == "gone").unwrap();
    assert!(gone.external, "missing repo: no metadata");
    let onfido = rows.iter().find(|r| r.name == "Onfido").unwrap();
    assert!(onfido.external);
    assert_eq!(onfido.html_url, "https://onfido.com");
    assert!(
        rows.windows(2).all(|w| w[0].rank < w[1].rank),
        "README order kept"
    );
}

#[tokio::test]
async fn list_fetch_anonymous_has_no_metadata_and_304_reuses_entries() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/o/awesome-example/readme"))
        .and(header("if-none-match", "\"l1\""))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/o/awesome-example/readme"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"l1\"")
                .set_body_string(LIST_MD),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let client = GitHubClient::new(server.uri(), None).unwrap();
    let (_dir, store) = store();

    let first = awesome::fetch_list(
        &client,
        &store,
        "o/awesome-example",
        false,
        &HashMap::new(),
        &mut noop(),
    )
    .await
    .unwrap();
    assert!(!first.meta_complete);
    assert!(awesome::to_rows(&first).iter().all(|r| r.external));

    let again = awesome::fetch_list(
        &client,
        &store,
        "o/awesome-example",
        false,
        &HashMap::new(),
        &mut noop(),
    )
    .await
    .unwrap();
    assert_eq!(again.items, first.items, "304: entries come from the cache");
}

#[tokio::test]
async fn list_fetch_rejects_invalid_repo_names() {
    let server = MockServer::start().await;
    let client = GitHubClient::new(server.uri(), None).unwrap();
    let (_dir, store) = store();
    assert!(matches!(
        awesome::fetch_list(
            &client,
            &store,
            "../../etc",
            false,
            &HashMap::new(),
            &mut noop()
        )
        .await,
        Err(GitHubError::InvalidRepo(_))
    ));
}

#[test]
fn selection_round_trips_and_freshness() {
    let (_dir, store) = store();
    assert_eq!(store.last_selection(), None);
    let sel = awesome::Selection {
        repo: "o/r".into(),
        label: "Node.js › Cross-Platform".into(),
    };
    store.save_selection(&sel).unwrap();
    assert_eq!(store.last_selection(), Some(sel));

    let now = Some(chrono::Utc::now());
    let old = Some(chrono::Utc::now() - chrono::Duration::days(10));
    assert!(awesome::is_fresh(now, 7));
    assert!(!awesome::is_fresh(old, 7));
    assert!(awesome::is_fresh(old, 0), "0 = never stale");
    assert!(!awesome::is_fresh(None, 7));
}

#[tokio::test]
async fn repos_meta_requires_a_token() {
    let server = MockServer::start().await;
    let client = GitHubClient::new(server.uri(), None).unwrap();
    assert!(matches!(
        client
            .fetch_repos_meta(&["o/r".to_string()], &mut noop())
            .await,
        Err(GitHubError::TokenRequired)
    ));
}

/// Counts the repositories looked up across all GraphQL requests, and
/// fails every request after the first `ok_requests` with a secondary rate
/// limit (403 + retry-after), like GitHub does.
struct CountingMeta {
    looked_up: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
    ok_requests: usize,
}

impl Respond for CountingMeta {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let n = self.requests.fetch_add(1, Ordering::SeqCst);
        if n >= self.ok_requests {
            return ResponseTemplate::new(403)
                .insert_header("retry-after", "60")
                .set_body_json(json!({"message": "You have exceeded a secondary rate limit."}));
        }
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        self.looked_up.fetch_add(
            body["variables"].as_object().unwrap().len() / 2,
            Ordering::SeqCst,
        );
        EchoMeta.respond(req)
    }
}

fn many_repo_list(n: usize) -> String {
    let items: String = (0..n)
        .map(|i| format!("- [r{i}](https://github.com/o/r{i}) - repo {i}\n"))
        .collect();
    format!(
        "# Awesome Many\n\n## Things\n\n{items}- [gone](https://github.com/o/gone-one) - deleted\n"
    )
}

#[tokio::test]
async fn list_fetch_keeps_partial_metadata_and_resumes_with_only_the_rest() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/o/awesome-many/readme"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"m1\"")
                .set_body_string(many_repo_list(249)),
        )
        .mount(&server)
        .await;
    let looked_up = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(AtomicUsize::new(0));
    // 250 repos = 3 batches; with 4 in flight all start, the first succeeds.
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(CountingMeta {
            looked_up: looked_up.clone(),
            requests: requests.clone(),
            ok_requests: 1,
        })
        .mount(&server)
        .await;
    let client = GitHubClient::new(server.uri(), Some("tk".into())).unwrap();
    let (_dir, store) = store();

    let mut warnings = Vec::new();
    let mut progress = |e: FetchEvent| {
        if let FetchEvent::Warning(w) = e {
            warnings.push(w)
        }
    };
    let first = awesome::fetch_list(
        &client,
        &store,
        "o/awesome-many",
        false,
        &HashMap::new(),
        &mut progress,
    )
    .await
    .unwrap();
    assert!(!first.meta_complete, "stopped by the rate limit");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let with_meta = first.items.iter().filter(|i| i.meta.is_some()).count();
    // Sorted, the first batch holds o/gone-one (missing) + 99 real repos.
    assert_eq!(
        with_meta,
        META_BATCH - 1,
        "the batch that succeeded is kept"
    );

    // Limit lifted: the next open looks up only the 150 still unknown.
    requests.store(0, Ordering::SeqCst);
    looked_up.store(0, Ordering::SeqCst);
    let resumed_client = client; // same client, budget restored below
    let server_ok = CountingMeta {
        looked_up: looked_up.clone(),
        requests: requests.clone(),
        ok_requests: usize::MAX,
    };
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/repos/o/awesome-many/readme"))
        .and(header("if-none-match", "\"m1\""))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(server_ok)
        .mount(&server)
        .await;
    let second = awesome::fetch_list(
        &resumed_client,
        &store,
        "o/awesome-many",
        false,
        &HashMap::new(),
        &mut noop(),
    )
    .await
    .unwrap();
    assert!(second.meta_complete);
    assert_eq!(looked_up.load(Ordering::SeqCst), 150, "only the rest");
    assert_eq!(second.missing, vec!["o/gone-one".to_string()]);
    assert_eq!(
        second.items.iter().filter(|i| i.meta.is_some()).count(),
        249
    );

    // A third open needs no lookup at all; the deleted repo is not retried.
    looked_up.store(0, Ordering::SeqCst);
    awesome::fetch_list(
        &resumed_client,
        &store,
        "o/awesome-many",
        false,
        &HashMap::new(),
        &mut noop(),
    )
    .await
    .unwrap();
    assert_eq!(looked_up.load(Ordering::SeqCst), 0);

    // A full refresh (force) looks everything up again.
    Mock::given(method("GET"))
        .and(path("/repos/o/awesome-many/readme"))
        .respond_with(ResponseTemplate::new(200).set_body_string(many_repo_list(249)))
        .mount(&server)
        .await;
    awesome::fetch_list(
        &resumed_client,
        &store,
        "o/awesome-many",
        true,
        &HashMap::new(),
        &mut noop(),
    )
    .await
    .unwrap();
    assert_eq!(looked_up.load(Ordering::SeqCst), 250);
}

#[tokio::test]
async fn starred_repos_cost_no_lookup() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/o/awesome-many/readme"))
        .respond_with(ResponseTemplate::new(200).set_body_string(many_repo_list(3)))
        .mount(&server)
        .await;
    let looked_up = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(CountingMeta {
            looked_up: looked_up.clone(),
            requests: Arc::new(AtomicUsize::new(0)),
            ok_requests: usize::MAX,
        })
        .mount(&server)
        .await;
    let client = GitHubClient::new(server.uri(), Some("tk".into())).unwrap();
    let (_dir, store) = store();

    let starred = stargazer::cache::StarredRepo {
        id: 42,
        full_name: "O/R1".into(), // case differs from the list's link
        stargazers_count: 777,
        html_url: "https://github.com/O/R1".into(),
        ..stargazer::cache::StarredRepo::test_default()
    };
    let known = awesome::known_from_stars(&[starred]);
    let list = awesome::fetch_list(
        &client,
        &store,
        "o/awesome-many",
        false,
        &known,
        &mut noop(),
    )
    .await
    .unwrap();
    assert_eq!(
        looked_up.load(Ordering::SeqCst),
        3,
        "r0, r2 and gone-one; r1 is starred"
    );
    let rows = awesome::to_rows(&list);
    assert_eq!(
        rows.iter().find(|r| r.id == 42).unwrap().stargazers_count,
        777
    );
}
