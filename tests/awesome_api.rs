//! GitHub client features used by the Awesome browser, against a mock server.

use serde_json::json;
use stargazer::github::{FetchEvent, GitHubClient, GitHubError, META_BATCH};
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
    let meta = client
        .fetch_repos_meta(&names, &mut progress)
        .await
        .unwrap();

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
    let meta = client.fetch_repos_meta(&names, &mut noop()).await.unwrap();
    assert_eq!(meta, vec![None, None]);
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
