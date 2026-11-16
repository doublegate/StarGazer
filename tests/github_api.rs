//! GitHub client and sync tests against a local mock server.

use serde_json::json;
use stargazer::cache::{StarCache, StarredRepo};
use stargazer::github::{FetchEvent, GitHubClient, GitHubError};
use stargazer::sync::{SyncMode, sync};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn rest_item(id: u64, starred: &str) -> serde_json::Value {
    json!({
        "starred_at": starred,
        "repo": {
            "id": id, "full_name": format!("o/r{id}"), "name": format!("r{id}"),
            "description": "d", "html_url": format!("https://github.com/o/r{id}"),
            "stargazers_count": id * 10, "language": "Rust", "topics": ["rust"],
            "updated_at": "2026-01-01T00:00:00Z", "pushed_at": null,
            "fork": false, "archived": false
        }
    })
}

fn gql_edge(id: u64) -> serde_json::Value {
    json!({
        "starredAt": "2026-03-01T00:00:00Z",
        "node": {
            "databaseId": id, "nameWithOwner": format!("o/r{id}"), "name": format!("r{id}"),
            "description": null, "url": format!("https://github.com/o/r{id}"),
            "stargazerCount": 1, "primaryLanguage": null,
            "repositoryTopics": {"nodes": []},
            "updatedAt": "2026-01-01T00:00:00Z", "pushedAt": null,
            "isFork": false, "isArchived": false
        }
    })
}

fn noop() -> impl FnMut(FetchEvent) + Send {
    |_| {}
}

#[tokio::test]
async fn rest_full_import_follows_link_pages() {
    let server = MockServer::start().await;
    let next = format!(
        "<{}/users/octocat/starred?page=2>; rel=\"next\"",
        server.uri()
    );
    Mock::given(method("GET"))
        .and(path("/users/octocat/starred"))
        .and(query_param("page", "1"))
        .and(header("accept", "application/vnd.github.star+json"))
        .and(header("x-github-api-version", "2022-11-28"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("link", next.as_str())
                .insert_header("etag", "\"abc\"")
                .set_body_json(json!([
                    rest_item(3, "2026-03-03T00:00:00Z"),
                    rest_item(2, "2026-03-02T00:00:00Z")
                ])),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users/octocat/starred"))
        .and(query_param("page", "2"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([rest_item(1, "2026-03-01T00:00:00Z")])),
        )
        .mount(&server)
        .await;

    let client = GitHubClient::new(server.uri(), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stars.json");
    let cache = sync(
        &client,
        "octocat",
        StarCache::default(),
        &path,
        SyncMode::Auto,
        7,
        &mut noop(),
    )
    .await
    .unwrap();

    let ids: Vec<u64> = cache.repos.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![3, 2, 1]);
    assert!(cache.complete);
    assert_eq!(cache.rest_etag.as_deref(), Some("\"abc\""));
    assert_eq!(cache.repos[0].category, "Rust");
    assert_eq!(StarCache::load_from(&path).unwrap().cache, cache);
}

#[tokio::test]
async fn rest_incremental_stops_at_known_and_handles_304() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/octocat/starred"))
        .and(header("if-none-match", "\"same\""))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&server)
        .await;

    let client = GitHubClient::new(server.uri(), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stars.json");
    let existing = StarCache {
        version: stargazer::cache::CACHE_VERSION,
        username: "octocat".into(),
        fetched_at: Some(chrono::Utc::now()),
        full_synced_at: Some(chrono::Utc::now()),
        complete: true,
        rules_version: stargazer::categories::RULES_VERSION,
        rest_etag: Some("\"same\"".into()),
        repos: vec![StarredRepo {
            id: 1,
            ..StarredRepo::test_default()
        }],
    };
    let cache = sync(
        &client,
        "octocat",
        existing,
        &path,
        SyncMode::Auto,
        7,
        &mut noop(),
    )
    .await
    .unwrap();
    assert_eq!(cache.repos.len(), 1);
}

#[tokio::test]
async fn rest_incremental_prepends_new_stars() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/octocat/starred"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            rest_item(5, "2026-05-01T00:00:00Z"),
            rest_item(4, "2026-04-01T00:00:00Z"),
            rest_item(1, "2026-01-01T00:00:00Z")
        ])))
        .mount(&server)
        .await;
    let client = GitHubClient::new(server.uri(), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let existing = StarCache {
        version: stargazer::cache::CACHE_VERSION,
        username: "octocat".into(),
        full_synced_at: Some(chrono::Utc::now()),
        complete: true,
        repos: vec![StarredRepo {
            id: 1,
            ..StarredRepo::test_default()
        }],
        ..Default::default()
    };
    let cache = sync(
        &client,
        "octocat",
        existing,
        &dir.path().join("s.json"),
        SyncMode::Auto,
        7,
        &mut noop(),
    )
    .await
    .unwrap();
    let ids: Vec<u64> = cache.repos.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![5, 4, 1]);
}

#[tokio::test]
async fn graphql_used_with_token_and_paginates() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(header("authorization", "Bearer t0ken"))
        .and(wiremock::matchers::body_partial_json(
            json!({"variables": {"after": null}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data": {"user": {"starredRepositories": {
                "pageInfo": {"hasNextPage": true, "endCursor": "c1"},
                "edges": [gql_edge(9), gql_edge(8)]
            }}}}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(wiremock::matchers::body_partial_json(
            json!({"variables": {"after": "c1"}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data": {"user": {"starredRepositories": {
                "pageInfo": {"hasNextPage": false, "endCursor": null},
                "edges": [gql_edge(7)]
            }}}}),
        ))
        .mount(&server)
        .await;

    let client = GitHubClient::new(server.uri(), Some("t0ken".into())).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = sync(
        &client,
        "octocat",
        StarCache::default(),
        &dir.path().join("s.json"),
        SyncMode::Full,
        7,
        &mut noop(),
    )
    .await
    .unwrap();
    let ids: Vec<u64> = cache.repos.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![9, 8, 7]);
}

#[tokio::test]
async fn graphql_sync_clears_an_outdated_rest_etag() {
    // agy review of PR #2: a REST ETag kept across GraphQL syncs could later
    // let an anonymous run accept a 304 against a stale state.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data": {"user": {"starredRepositories": {
                "pageInfo": {"hasNextPage": false, "endCursor": null},
                "edges": [gql_edge(7)]
            }}}}),
        ))
        .mount(&server)
        .await;
    let client = GitHubClient::new(server.uri(), Some("t0ken".into())).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let before = StarCache {
        username: "octocat".into(),
        rest_etag: Some("\"old\"".into()),
        ..Default::default()
    };
    let cache = sync(
        &client,
        "octocat",
        before,
        &dir.path().join("s.json"),
        SyncMode::Full,
        7,
        &mut noop(),
    )
    .await
    .unwrap();
    assert_eq!(cache.rest_etag, None);
}

#[tokio::test]
async fn full_sync_drops_unstarred_repos() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/octocat/starred"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([rest_item(2, "2026-02-01T00:00:00Z")])),
        )
        .mount(&server)
        .await;
    let client = GitHubClient::new(server.uri(), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let existing = StarCache {
        username: "octocat".into(),
        complete: true,
        repos: vec![
            StarredRepo {
                id: 2,
                ..StarredRepo::test_default()
            },
            StarredRepo {
                id: 1,
                ..StarredRepo::test_default()
            },
        ],
        ..Default::default()
    };
    let cache = sync(
        &client,
        "octocat",
        existing,
        &dir.path().join("s.json"),
        SyncMode::Full,
        7,
        &mut noop(),
    )
    .await
    .unwrap();
    assert_eq!(
        cache.repos.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![2]
    );
}

#[tokio::test]
async fn errors_are_typed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/ghost/starred"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"message": "Not Found"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users/limited/starred"))
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header("x-ratelimit-remaining", "0")
                .insert_header("x-ratelimit-reset", "1800000000")
                .set_body_json(json!({"message": "API rate limit exceeded"})),
        )
        .mount(&server)
        .await;

    let client = GitHubClient::new(server.uri(), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("s.json");

    let e = sync(
        &client,
        "ghost",
        StarCache::default(),
        &p,
        SyncMode::Auto,
        7,
        &mut noop(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(e.error, GitHubError::UserNotFound(ref u) if u == "ghost"),
        "{:?}",
        e.error
    );

    let e = sync(
        &client,
        "limited",
        StarCache::default(),
        &p,
        SyncMode::Auto,
        7,
        &mut noop(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(e.error, GitHubError::RateLimited { reset: Some(_) }),
        "{:?}",
        e.error
    );
    assert!(e.error.to_string().contains("resets at"));

    let e = sync(
        &client,
        "bad/../user",
        StarCache::default(),
        &p,
        SyncMode::Auto,
        7,
        &mut noop(),
    )
    .await
    .unwrap_err();
    assert!(matches!(e.error, GitHubError::InvalidUsername(_)));
}

#[tokio::test]
async fn server_errors_are_retried() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/octocat/starred"))
        .respond_with(ResponseTemplate::new(502))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users/octocat/starred"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([rest_item(1, "2026-01-01T00:00:00Z")])),
        )
        .mount(&server)
        .await;
    let client = GitHubClient::new(server.uri(), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = sync(
        &client,
        "octocat",
        StarCache::default(),
        &dir.path().join("s.json"),
        SyncMode::Auto,
        7,
        &mut noop(),
    )
    .await
    .unwrap();
    assert_eq!(cache.repos.len(), 1);
}

#[tokio::test]
async fn partial_import_is_kept_on_failure() {
    let server = MockServer::start().await;
    let next = format!(
        "<{}/users/octocat/starred?page=2>; rel=\"next\"",
        server.uri()
    );
    Mock::given(method("GET"))
        .and(path("/users/octocat/starred"))
        .and(query_param("page", "1"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("link", next.as_str())
                .set_body_json(json!([rest_item(2, "2026-02-01T00:00:00Z")])),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users/octocat/starred"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let client = GitHubClient::new(server.uri(), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("s.json");
    let e = sync(
        &client,
        "octocat",
        StarCache::default(),
        &p,
        SyncMode::Auto,
        7,
        &mut noop(),
    )
    .await
    .unwrap_err();
    assert!(matches!(e.error, GitHubError::Unauthorized));
    let partial = e.partial.expect("partial cache");
    assert!(!partial.complete);
    assert_eq!(partial.repos.len(), 1);
    assert!(
        !StarCache::load_from(&p).unwrap().cache.complete,
        "next sync must be full"
    );
}

#[tokio::test]
async fn readme_and_unstar() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/o/has/readme"))
        .and(header("accept", "application/vnd.github.raw+json"))
        .respond_with(ResponseTemplate::new(200).set_body_string("# Hello"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/o/none/readme"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/user/starred/o/has"))
        .and(header("authorization", "Bearer tk"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let anon = GitHubClient::new(server.uri(), None).unwrap();
    assert_eq!(
        anon.fetch_readme("o/has").await.unwrap().as_deref(),
        Some("# Hello")
    );
    assert_eq!(anon.fetch_readme("o/none").await.unwrap(), None);
    assert!(matches!(
        anon.unstar("o/has").await,
        Err(GitHubError::TokenRequired)
    ));
    assert!(matches!(
        anon.fetch_readme("o/../x").await,
        Err(GitHubError::InvalidRepo(_))
    ));

    let authed = GitHubClient::new(server.uri(), Some("tk".into())).unwrap();
    authed.unstar("o/has").await.unwrap();
}

fn complete_cache(ids: &[u64], etag: Option<&str>) -> StarCache {
    StarCache {
        version: stargazer::cache::CACHE_VERSION,
        username: "octocat".into(),
        fetched_at: Some(chrono::Utc::now()),
        full_synced_at: Some(chrono::Utc::now()),
        complete: true,
        rules_version: stargazer::categories::RULES_VERSION,
        rest_etag: etag.map(str::to_string),
        repos: ids
            .iter()
            .map(|&id| StarredRepo {
                id,
                ..StarredRepo::test_default()
            })
            .collect(),
    }
}

#[tokio::test]
async fn rest_incremental_stops_paging_at_first_known_star() {
    let server = MockServer::start().await;
    let next = format!(
        "<{}/users/octocat/starred?page=2>; rel=\"next\"",
        server.uri()
    );
    // Page 1 contains a known id (1) after one new star; page 2 must never be
    // requested.
    Mock::given(method("GET"))
        .and(path("/users/octocat/starred"))
        .and(query_param("page", "1"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("link", next.as_str())
                .set_body_json(json!([
                    rest_item(2, "2026-02-01T00:00:00Z"),
                    rest_item(1, "2026-01-01T00:00:00Z")
                ])),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users/octocat/starred"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let client = GitHubClient::new(server.uri(), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = sync(
        &client,
        "octocat",
        complete_cache(&[1], None),
        &dir.path().join("s.json"),
        SyncMode::Auto,
        7,
        &mut noop(),
    )
    .await
    .unwrap();
    assert_eq!(
        cache.repos.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![2, 1]
    );
}

#[tokio::test]
async fn full_sync_never_sends_if_none_match() {
    let server = MockServer::start().await;
    // A conditional request would get 304 and silently skip the full resync.
    Mock::given(method("GET"))
        .and(path("/users/octocat/starred"))
        .and(header("if-none-match", "\"old\""))
        .respond_with(ResponseTemplate::new(304))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users/octocat/starred"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([rest_item(9, "2026-09-01T00:00:00Z")])),
        )
        .mount(&server)
        .await;
    let client = GitHubClient::new(server.uri(), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = sync(
        &client,
        "octocat",
        complete_cache(&[1], Some("\"old\"")),
        &dir.path().join("s.json"),
        SyncMode::Full,
        7,
        &mut noop(),
    )
    .await
    .unwrap();
    assert_eq!(
        cache.repos.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![9]
    );
}

#[tokio::test]
async fn graphql_partial_errors_skip_entries_but_keep_the_rest() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {"user": {"starredRepositories": {
                "pageInfo": {"hasNextPage": false, "endCursor": null},
                "edges": [gql_edge(5), {"starredAt": "2026-03-01T00:00:00Z", "node": null}, gql_edge(4)]
            }}},
            "errors": [{"type": "FORBIDDEN", "message": "Resource protected by organization SAML enforcement"}]
        })))
        .mount(&server)
        .await;
    let client = GitHubClient::new(server.uri(), Some("t".into())).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut warnings = Vec::new();
    let mut progress = |ev: FetchEvent| {
        if let FetchEvent::Warning(w) = ev {
            warnings.push(w);
        }
    };
    let cache = sync(
        &client,
        "octocat",
        StarCache::default(),
        &dir.path().join("s.json"),
        SyncMode::Full,
        7,
        &mut progress,
    )
    .await
    .unwrap();
    assert_eq!(
        cache.repos.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![5, 4]
    );
    assert!(warnings.iter().any(|w| w.contains("SAML")), "{warnings:?}");
}

#[tokio::test]
async fn rest_page_of_only_malformed_entries_does_not_end_import() {
    let server = MockServer::start().await;
    let next = format!(
        "<{}/users/octocat/starred?page=2>; rel=\"next\"",
        server.uri()
    );
    Mock::given(method("GET"))
        .and(path("/users/octocat/starred"))
        .and(query_param("page", "1"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("link", next.as_str())
                .set_body_json(json!([{"starred_at": "x", "repo": {"id": "bad"}}])),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users/octocat/starred"))
        .and(query_param("page", "2"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([rest_item(3, "2026-01-01T00:00:00Z")])),
        )
        .mount(&server)
        .await;
    let client = GitHubClient::new(server.uri(), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = sync(
        &client,
        "octocat",
        StarCache::default(),
        &dir.path().join("s.json"),
        SyncMode::Auto,
        7,
        &mut noop(),
    )
    .await
    .unwrap();
    assert_eq!(
        cache.repos.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![3]
    );
}

#[tokio::test]
async fn cached_username_case_difference_keeps_cache() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/OctoCat/starred"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    let client = GitHubClient::new(server.uri(), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = sync(
        &client,
        "OctoCat",
        complete_cache(&[1, 2], Some("\"e\"")),
        &dir.path().join("s.json"),
        SyncMode::Auto,
        7,
        &mut noop(),
    )
    .await
    .unwrap();
    assert_eq!(
        cache.repos.len(),
        2,
        "cache for 'octocat' must survive 'OctoCat'"
    );
}
