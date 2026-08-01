use super::*;
use axum::middleware;
use axum::routing::get;
use std::net::SocketAddr;

// Build the dashboard + admin routes against an AppState whose `admin_access` is
// injected directly (a bearer token). This avoids mutating the process env
// (`LLM_UNIVERSAL_PROXY_ADMIN_TOKEN`), which the production server bootstrap
// reads via `AdminAccess::from_env()` and which would race with the many
// reader tests that observe env. The dashboard routes are public; the admin
// route is guarded by `require_admin_access`, exactly as in the live router.
async fn start_dashboard_proxy(admin_token: &str) -> (String, tokio::task::JoinHandle<()>) {
    let config = crate::config::Config::default();
    let data_access = data_auth::DataAccess::ClientProviderKey;
    let runtime = crate::server::state::build_runtime_state(config.clone(), &data_access)
        .await
        .expect("build dashboard runtime");
    let state = Arc::new(AppState {
        runtime: Arc::new(RwLock::new(runtime)),
        admin_update_lock: Arc::new(Mutex::new(())),
        metrics: crate::telemetry::RuntimeMetrics::new(&config),
        admin_access: AdminAccess::BearerToken(admin_token.to_string()),
        data_auth_policy: test_data_auth_policy_for_tests(),
        conversation_state_bridge: Arc::new(
            crate::server::conversation_state_bridge::ConversationStateBridgeStore::new(),
        ),
    });

    let admin_router = Router::new()
        .route(
            "/admin/state",
            get(crate::server::admin::handle_admin_state),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            crate::server::admin::require_admin_access,
        ));

    let app = Router::new()
        .route(
            "/dashboard",
            get(crate::server::web_dashboard::handle_dashboard_index),
        )
        .route(
            "/dashboard/",
            get(crate::server::web_dashboard::handle_dashboard_index),
        )
        .route(
            "/dashboard/assets/app.css",
            get(crate::server::web_dashboard::handle_dashboard_css),
        )
        .route(
            "/dashboard/assets/app.js",
            get(crate::server::web_dashboard::handle_dashboard_js),
        )
        .merge(admin_router)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind dashboard proxy");
    let addr = listener.local_addr().expect("dashboard local addr");
    let base = format!("http://127.0.0.1:{}", addr.port());
    let handle = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("dashboard proxy server");
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (base, handle)
}

fn dashboard_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("dashboard test client")
}

#[tokio::test]
async fn dashboard_shell_is_public_when_admin_token_is_configured() {
    let (proxy_base, _proxy) = start_dashboard_proxy("dashboard-secret").await;
    let client = dashboard_client();

    let response = client
        .get(format!("{proxy_base}/dashboard"))
        .header("origin", "https://example.com")
        .send()
        .await
        .expect("dashboard response");

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response
        .headers()
        .get("access-control-allow-origin")
        .is_none());
    assert!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html")),
        "dashboard index should be served as HTML"
    );
    let body = response.text().await.expect("dashboard body");
    assert!(body.contains("LLM Universal Proxy Admin"));
    assert!(body.contains("Admin token"));
    assert!(body.contains("Bearer token"));
    assert!(body.contains("existing admin API"));
    assert!(body.contains("placeholder=\"Paste admin token\""));
    assert!(!body.contains("placeholder=\"Paste LLM_UNIVERSAL_PROXY_ADMIN_TOKEN\""));
    assert!(!body.contains("compatibility_mode"));
}

#[tokio::test]
async fn dashboard_static_assets_are_public_shell_resources_with_content_types() {
    let (proxy_base, _proxy) = start_dashboard_proxy("dashboard-secret").await;
    let client = dashboard_client();

    let js = client
        .get(format!("{proxy_base}/dashboard/assets/app.js"))
        .send()
        .await
        .expect("dashboard app script response");
    assert_eq!(js.status(), StatusCode::OK);
    assert!(
        js.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/javascript")),
        "dashboard app script should be served as JavaScript"
    );
    let js_body = js.text().await.expect("asset body");
    assert!(js_body.contains("DashboardClient"));
    assert!(js_body.contains("Authorization"));
    assert!(js_body.contains("/admin/state"));
    assert!(js_body.contains("/admin/namespaces/"));

    let css = client
        .get(format!("{proxy_base}/dashboard/assets/app.css"))
        .header("origin", "https://example.com")
        .send()
        .await
        .expect("dashboard stylesheet response");
    assert_eq!(css.status(), StatusCode::OK);
    assert!(css.headers().get("access-control-allow-origin").is_none());
    assert!(
        css.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/css")),
        "dashboard stylesheet should be served as CSS"
    );
    let css_body = css.text().await.expect("css body");
    assert!(css_body.contains(".dashboard"));
    assert!(css_body.contains("@media (max-width: 640px)"));
    assert!(css_body.contains("width: calc(100vw - 3rem)"));
    assert!(css_body.contains("max-width: calc(100vw - 3rem)"));
    assert!(css_body.contains("margin-left: 1rem"));
    assert!(css_body.contains("overflow-x: hidden"));
    assert!(css_body.contains("min-width: 0"));
    assert!(css_body.contains("overflow-wrap: anywhere"));
    assert!(css_body.contains("word-break: break-word"));
    assert!(css_body.contains("font-size: clamp(1.75rem, 10vw, 2.35rem)"));
    assert!(css_body.contains(".auth-panel p"));
    assert!(css_body.contains(".namespace-card span"));
    assert!(css_body.contains("justify-items: stretch"));
    assert!(css_body.contains("width: 100%"));
    assert!(css_body.contains(".auth-panel > *"));
    assert!(css_body.contains(".auth-row button"));
}

#[tokio::test]
async fn admin_endpoints_still_require_bearer_when_dashboard_shell_is_public() {
    let (proxy_base, _proxy) = start_dashboard_proxy("dashboard-secret").await;
    let client = dashboard_client();

    let dashboard = client
        .get(format!("{proxy_base}/dashboard"))
        .send()
        .await
        .expect("dashboard shell response");
    assert_eq!(dashboard.status(), StatusCode::OK);

    let missing = client
        .get(format!("{proxy_base}/admin/state"))
        .send()
        .await
        .expect("missing admin token response");
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

    let wrong = client
        .get(format!("{proxy_base}/admin/state"))
        .header("authorization", "Bearer wrong-token")
        .send()
        .await
        .expect("wrong admin token response");
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

    let admin = client
        .get(format!("{proxy_base}/admin/state"))
        .header("authorization", "Bearer dashboard-secret")
        .send()
        .await
        .expect("authorized admin state response");
    assert_eq!(admin.status(), StatusCode::OK);
    let body: serde_json::Value = admin.json().await.expect("admin state json");
    assert!(body["namespaces"].is_array());
}

#[tokio::test]
async fn dashboard_copy_keeps_redacted_state_read_only_and_requires_full_payload() {
    let (proxy_base, _proxy) = start_dashboard_proxy("dashboard-secret").await;
    let client = dashboard_client();

    let body = client
        .get(format!("{proxy_base}/dashboard"))
        .send()
        .await
        .expect("dashboard response")
        .text()
        .await
        .expect("dashboard body");

    assert!(body.contains("Redacted State"));
    assert!(body.contains("Paste a complete runtime config payload"));
    assert!(body.contains("Do not submit redacted state from above"));
    assert!(body.contains("redacted secrets are intentionally not editable payloads"));
}
