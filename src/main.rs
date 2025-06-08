use crate::shared_queries::*;
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use argument_parsing::Args;
use askama::Template;
use askama_axum::IntoResponse as AskamaIntoResponse;
use axum::{
    Form, Router,
    body::{Body, Bytes},
    extract::{Path, State},
    response::{IntoResponse as AxumIntoResponse, Redirect, Response},
    routing::{get, post},
};
use chrono::{DateTime, Timelike, Utc};
use clap::Parser;
use futures_util::StreamExt;
use jsonwebtoken::errors::Error;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, SqlitePool, migrate::Migrator};
use tokio::{
    signal,
    time::{self, Duration},
};
use tower_http::trace::TraceLayer;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use validator::Validate;

mod argument_parsing;
mod postgres_queries;
mod shared_queries;
mod sqlite;
mod sqlite_queries;

#[derive(Serialize, Template)]
#[template(path = "registration.html")]
struct RegistrationTemplate {
    logged_in: bool,
}

#[derive(Deserialize)]
struct RegistrationInput {
    username: String,
    password: String,
}

#[derive(sqlx::FromRow)]
struct User {
    username: String,
    password_hash: String,
    salt: String,
}

#[derive(Deserialize, sqlx::FromRow, Validate)]
struct Website {
    #[validate(url)]
    url: String,
    alias: String,
}

#[derive(Serialize, Validate)]
struct WebsiteInfo {
    #[validate(url)]
    url: String,
    alias: String,
    data: Vec<WebsiteStats>,
}

#[derive(sqlx::FromRow, Serialize)]
pub struct WebsiteStats {
    time: DateTime<Utc>,
    uptime_pct: Option<i16>,
}

#[derive(Serialize, sqlx::FromRow)]
struct WebsiteLogs {
    logs: Vec<WebsiteInfo>,
}

#[derive(Serialize, Template)]
#[template(path = "websites.html")]
struct WebsiteLogsTemplate {
    logged_in: bool,
    logs: Vec<WebsiteInfo>,
}

#[derive(Serialize, Template)]
#[template(path = "websites-fragment.html")]
struct WebsiteLogsFragmentTemplate {
    logged_in: bool,
    logs: Vec<WebsiteInfo>,
}

#[derive(Serialize, sqlx::FromRow)]
struct SingleWebsiteLog {
    log: WebsiteInfo,
    incidents: Vec<Incident>,
    monthly_data: Vec<WebsiteStats>,
}

#[derive(Serialize, Template)]
#[template(path = "single_website.html")]
struct SingleWebsiteLogTemplate {
    logged_in: bool,
    log: WebsiteInfo,
    incidents: Vec<Incident>,
    monthly_data: Vec<WebsiteStats>,
}

#[derive(Serialize, sqlx::FromRow)]
struct Incident {
    time: DateTime<Utc>,
    status: i16,
}

#[derive(Clone, Debug)]
enum AppState {
    Postgres(PgPool),
    Sqlite(SqlitePool),
}

impl AppState {
    fn new(postgres: Option<PgPool>, sqlite: Option<SqlitePool>) -> Self {
        match (postgres, sqlite) {
            (Some(p), _) => AppState::Postgres(p),
            (_, Some(s)) => AppState::Sqlite(s),
            _ => panic!("You need to configure either Postgres or Sqlite!"),
        }
    }

    async fn migrate_db(&self) {
        match self {
            Self::Postgres(p) => Self::migrate_postgres(p).await,
            Self::Sqlite(s) => sqlite::migrate_sqlite(s).await,
        }
    }

    async fn migrate_postgres(pool: &PgPool) {
        let migrator = Migrator::new(std::path::Path::new("./migrations_pg"))
            .await
            .expect("Migration folder couldn't be found");
        migrator
            .run(pool)
            .await
            .expect("Postgres migrations failed");
    }

    async fn from(item: argument_parsing::Args) -> Self {
        if let Some(pg_string) = item.pg {
            if pg_string.is_empty() {
                AppState::new(
                    None,
                    Some(SqlitePool::connect(SQLITE_CONNECTION_STRING).await.unwrap()),
                )
            } else {
                AppState::new(Some(PgPool::connect(&pg_string).await.unwrap()), None)
            }
        } else {
            AppState::new(
                None,
                Some(SqlitePool::connect(SQLITE_CONNECTION_STRING).await.unwrap()),
            )
        }
    }
}

enum ApiError {
    SQL(sqlx::Error),
    Validation(String),
    PasswordHashing(argon2::password_hash::Error),
    JsonWebTokenError(jsonwebtoken::errors::Error),
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        Self::SQL(e)
    }
}

impl From<String> for ApiError {
    fn from(value: String) -> Self {
        Self::Validation(value)
    }
}

impl From<argon2::password_hash::Error> for ApiError {
    fn from(value: argon2::password_hash::Error) -> Self {
        Self::PasswordHashing(value)
    }
}

impl From<jsonwebtoken::errors::Error> for ApiError {
    fn from(value: jsonwebtoken::errors::Error) -> Self {
        Self::JsonWebTokenError(value)
    }
}

impl AxumIntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::SQL(e) => AxumIntoResponse::into_response((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("SQL Error: {e}"),
            )),
            Self::Validation(s) => AxumIntoResponse::into_response((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Validation Error: {s}"),
            )),
            Self::PasswordHashing(a) => AxumIntoResponse::into_response((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Password Hashing Error: {a}"),
            )),
            Self::JsonWebTokenError(a) => AxumIntoResponse::into_response((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Json Web Token Error: {a}"),
            )),
        }
    }
}

const SQLITE_CONNECTION_STRING: &'static str = "sqlite://uptime_ferris.db?mode=rwc";

#[tokio::main]
async fn main() {
    //Init tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                // axum logs rejections from built-in extractors with the `axum::rejection`
                // target, at `TRACE` level. `axum::rejection=trace` enables showing those events
                format!(
                    "{}=debug,tower_http=debug,axum::rejection=trace",
                    env!("CARGO_CRATE_NAME")
                )
                .into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let args = Args::parse();
    let app_state = AppState::from(args).await;
    // carry out migrations
    info!("Starting db migration");
    let _ = &app_state.migrate_db().await;
    info!("Finished db migration");
    let cloned_state = app_state.clone();
    //Check the website status
    info!("Starting background task for checking website status");
    tokio::spawn(async move {
        check_websites_general(cloned_state).await;
    });

    // build our application with a route
    let app = Router::new()
        .route("/health", get(health_check))
        .route("/registration", get(get_registration))
        .route("/register", post(register_user))
        .route("/websites", get(get_websites))
        .route("/websites", post(create_website))
        .route(
            "/websites/:alias",
            get(get_website_by_alias).delete(delete_website),
        )
        .route("/assets/logo.svg", get(logo))
        .route("/assets/favicon-96x96.png", get(favicon_96_png))
        .route("/assets/favicon.svg", get(favicon_svg))
        .route("/assets/favicon.ico", get(favicon_ico))
        .route("/assets/apple-touch-icon.png", get(apple_touch_icon))
        .route("/assets/site.webmanifest", get(site_webmanifest))
        .route(
            "/assets/web-app-manifest-192x192.png",
            get(web_app_manifest_192),
        )
        .route(
            "/assets/web-app-manifest-512x512.png",
            get(web_app_manifest_512),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(app_state);

    // run it
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();
    info!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();
}

async fn health_check() -> impl AxumIntoResponse {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/plain")
        .body("healthy".to_owned())
        .unwrap()
}

macro_rules! included_text_content_handler {
    ($name:ident,$content_type:literal,$path:literal) => {
        async fn $name() -> impl AxumIntoResponse {
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", $content_type)
                .body(include_str!($path).to_owned())
                .unwrap()
        }
    };
}

included_text_content_handler!(
    site_webmanifest,
    "application/webmanifest+json",
    "../assets/site.webmanifest"
);
included_text_content_handler!(logo, "image/svg+xml", "../assets/uptime_ferris_logo.svg");
included_text_content_handler!(favicon_svg, "image/svg+xml", "../assets/favicon.svg");

macro_rules! included_binary_content_handler {
    ($name:ident,$content_type:literal,$path:literal) => {
        async fn $name() -> impl AxumIntoResponse {
            let body: Body = Bytes::from_static(include_bytes!($path)).into();
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", $content_type)
                .body(body)
                .unwrap()
        }
    };
}

included_binary_content_handler!(
    apple_touch_icon,
    "image/png",
    "../assets/apple-touch-icon.png"
);
included_binary_content_handler!(favicon_96_png, "image/png", "../assets/favicon-96x96.png");
included_binary_content_handler!(
    web_app_manifest_192,
    "image/png",
    "../assets/web-app-manifest-192x192.png"
);
included_binary_content_handler!(
    web_app_manifest_512,
    "image/png",
    "../assets/web-app-manifest-512x512.png"
);
included_binary_content_handler!(
    favicon_ico,
    "image/vnd.microsoft.icon",
    "../assets/favicon.ico"
);

async fn create_website(
    State(state): State<AppState>,
    Form(new_website): Form<Website>,
) -> Result<impl AskamaIntoResponse, ApiError> {
    if new_website.validate().is_err() {
        return Err(String::from("Validation Error: is your website a reachable URL?").into());
    }

    match state {
        AppState::Postgres(ref p) => {
            let _ = sqlx::query(INSERT_INTO_WEBSITES_QUERY)
                .bind(new_website.url)
                .bind(new_website.alias)
                .bind(Utc::now())
                .execute(p)
                .await?;
        }
        AppState::Sqlite(ref s) => {
            let _ = sqlx::query(INSERT_INTO_WEBSITES_QUERY)
                .bind(new_website.url)
                .bind(new_website.alias)
                .bind(Utc::now())
                .execute(s)
                .await?;
        }
    }

    let logs = get_website_logs(state).await?;

    Ok(WebsiteLogsFragmentTemplate {
        logged_in: false,
        logs,
    })
}

async fn get_website_logs(state: AppState) -> Result<Vec<WebsiteInfo>, ApiError> {
    let websites = match state {
        AppState::Postgres(ref p) => {
            sqlx::query_as::<_, Website>(SELECT_URL_ALIAS_WEBSITES_QUERY)
                .fetch_all(p)
                .await?
        }
        AppState::Sqlite(ref s) => {
            sqlx::query_as::<_, Website>(SELECT_URL_ALIAS_WEBSITES_QUERY)
                .fetch_all(s)
                .await?
        }
    };
    let mut logs = Vec::new();

    for website in websites {
        let data = get_daily_stats(&website.alias, &state).await?;

        logs.push(WebsiteInfo {
            url: website.url,
            alias: website.alias,
            data,
        })
    }

    Ok(logs)
}

async fn get_registration() -> impl AskamaIntoResponse {
    RegistrationTemplate { logged_in: false }
}

async fn register_user(
    State(state): State<AppState>,
    Form(registration_input): Form<RegistrationInput>,
) -> Result<impl AxumIntoResponse, ApiError> {
    let hashed_password = hash_password(&registration_input.password)?;
    let user = User {
        username: registration_input.username,
        password_hash: hashed_password.hash,
        salt: hashed_password.salt,
    };

    match state {
        AppState::Postgres(pool) => {
            let _ =
                sqlx::query("INSERT INTO Users (username, password_hash, salt) VALUES ($1,$2,$3)")
                    .bind(user.username)
                    .bind(user.password_hash)
                    .bind(user.salt)
                    .execute(&pool)
                    .await?;
        }
        AppState::Sqlite(pool) => {
            let _ =
                sqlx::query("INSERT INTO Users (username, password_hash, salt) VALUES ($1,$2,$3)")
                    .bind(user.username)
                    .bind(user.password_hash)
                    .bind(user.salt)
                    .execute(&pool)
                    .await?;
        }
    };

    Ok(Redirect::to("/websites"))
}

struct HashedPassword {
    hash: String,
    salt: String,
}

fn hash_password(password: &str) -> Result<HashedPassword, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2.hash_password(password.as_bytes(), &salt)?;

    Ok(HashedPassword {
        hash: password_hash.to_string(),
        salt: salt.to_string(),
    })
}

fn verify_password(user: &User, password: &str) -> Result<bool, argon2::password_hash::Error> {
    let parsed_hash = PasswordHash::new(&user.password_hash)?;
    let result = Argon2::default().verify_password(password.as_bytes(), &parsed_hash);

    Ok(result.is_ok())
}

#[axum::debug_handler]
async fn get_websites(State(state): State<AppState>) -> Result<impl AskamaIntoResponse, ApiError> {
    let logs = get_website_logs(state).await?;
    Ok(WebsiteLogsTemplate {
        logs,
        logged_in: false,
    })
}

enum SplitBy {
    Hour,
    Day,
}

async fn get_daily_stats(alias: &str, app_state: &AppState) -> Result<Vec<WebsiteStats>, ApiError> {
    let data = match app_state {
        AppState::Postgres(p) => {
            sqlx::query_as::<_, WebsiteStats>(postgres_queries::SELECT_DAILY_STATS)
                .bind(alias)
                .fetch_all(p)
                .await?
        }
        AppState::Sqlite(s) => {
            sqlx::query_as::<_, WebsiteStats>(sqlite_queries::SELECT_DAILY_STATS)
                .bind(alias)
                .fetch_all(s)
                .await?
        }
    };

    let number_of_splits = 24;
    let number_of_seconds = 3600;

    let data = fill_data_gaps(data, number_of_splits, SplitBy::Hour, number_of_seconds);

    Ok(data)
}

async fn get_monthly_stats(
    alias: &str,
    app_state: &AppState,
) -> Result<Vec<WebsiteStats>, ApiError> {
    let data = match app_state {
        AppState::Postgres(p) => {
            sqlx::query_as::<_, WebsiteStats>(postgres_queries::SELECT_MONTHLY_STATS)
                .bind(alias)
                .fetch_all(p)
                .await?
        }
        AppState::Sqlite(s) => {
            sqlx::query_as::<_, WebsiteStats>(sqlite_queries::SELECT_MONTHLY_STATS)
                .bind(alias)
                .fetch_all(s)
                .await?
        }
    };

    let number_of_splits = 30;
    let number_of_seconds = 86400;

    let data = fill_data_gaps(data, number_of_splits, SplitBy::Day, number_of_seconds);
    Ok(data)
}

fn fill_data_gaps(
    mut data: Vec<WebsiteStats>,
    splits: i32,
    format: SplitBy,
    number_of_seconds: i32,
) -> Vec<WebsiteStats> {
    let bound = match format {
        SplitBy::Hour => 24,
        SplitBy::Day => 30,
    };
    // If the length of data is not as long as the number of required splits (24)
    // then we fill in the gaps
    if (data.len() as i32) < splits {
        for i in 0..bound {
            let time = Utc::now() - chrono::Duration::seconds((number_of_seconds * i).into());
            let time = time
                .with_minute(0)
                .unwrap()
                .with_second(0)
                .unwrap()
                .with_nanosecond(0)
                .unwrap();

            let time = if matches!(format, SplitBy::Day) {
                time.with_hour(0).unwrap()
            } else {
                time
            };

            // if timestamp doesn't exist, push a timestamp with None
            if !data.iter().any(|x| x.time == time) {
                data.push(WebsiteStats {
                    time,
                    uptime_pct: None,
                });
            }
        }
        // finally, sort the data
        data.sort_by(|a, b| b.time.cmp(&a.time));
    }

    data
}

#[axum::debug_handler]
async fn get_website_by_alias(
    State(state): State<AppState>,
    Path(alias): Path<String>,
) -> Result<impl AskamaIntoResponse, ApiError> {
    info!("retrieving website entry for alias");
    let website = match state {
        AppState::Postgres(ref p) => {
            sqlx::query_as::<_, Website>(SELECT_URL_ALIAS_WEBSITES_TOP_ONE_WHERE_ALIAS_QUERY)
                .bind(&alias)
                .fetch_one(p)
                .await?
        }
        AppState::Sqlite(ref s) => {
            sqlx::query_as::<_, Website>(SELECT_URL_ALIAS_WEBSITES_TOP_ONE_WHERE_ALIAS_QUERY)
                .bind(&alias)
                .fetch_one(s)
                .await?
        }
    };

    info!("Getting stats for last 24h");
    let last_24_hours_data = get_daily_stats(&website.alias, &state).await?;
    info!("Getting monthly data");
    let monthly_data = get_monthly_stats(&website.alias, &state).await?;

    info!("Getting incidents");
    let incidents = match state {
        AppState::Postgres(p) => {
            sqlx::query_as::<_, Incident>(SELECT_INCIDENTS_BY_WEBSITE_ALIAS_QUERY)
                .bind(&alias)
                .fetch_all(&p)
                .await?
        }
        AppState::Sqlite(s) => {
            sqlx::query_as::<_, Incident>(SELECT_INCIDENTS_BY_WEBSITE_ALIAS_QUERY)
                .bind(&alias)
                .fetch_all(&s)
                .await?
        }
    };

    let log = WebsiteInfo {
        url: website.url,
        alias,
        data: last_24_hours_data,
    };

    Ok(SingleWebsiteLogTemplate {
        log,
        incidents,
        monthly_data,
        logged_in: false,
    })
}

async fn delete_website(
    State(state): State<AppState>,
    Path(alias): Path<String>,
) -> Result<impl AxumIntoResponse, ApiError> {
    match state {
        AppState::Postgres(p) => delete_website_postgres(&alias, p).await?,
        AppState::Sqlite(s) => delete_website_sqlite(&alias, s).await?,
    };

    Ok(StatusCode::OK)
}

async fn delete_website_postgres(alias: &str, db: PgPool) -> Result<(), ApiError> {
    let mut tx = db.begin().await?;
    if let Err(e) = sqlx::query(DELETE_LOGS_BY_WEBSITE_ALIAS_QUERY)
        .bind(alias)
        .execute(&mut *tx)
        .await
    {
        tx.rollback().await?;
        return Err(ApiError::SQL(e));
    };

    if let Err(e) = sqlx::query(DELETE_WEBSITE_BY_ALIAS_QUERY)
        .bind(alias)
        .execute(&mut *tx)
        .await
    {
        tx.rollback().await?;
        return Err(ApiError::SQL(e));
    }

    tx.commit().await?;

    Ok(())
}

async fn delete_website_sqlite(alias: &str, db: SqlitePool) -> Result<(), ApiError> {
    let mut tx = db.begin().await?;
    if let Err(e) = sqlx::query(DELETE_LOGS_BY_WEBSITE_ALIAS_QUERY)
        .bind(alias)
        .execute(&mut *tx)
        .await
    {
        tx.rollback().await?;
        return Err(ApiError::SQL(e));
    };

    if let Err(e) = sqlx::query(DELETE_WEBSITE_BY_ALIAS_QUERY)
        .bind(alias)
        .execute(&mut *tx)
        .await
    {
        tx.rollback().await?;
        return Err(ApiError::SQL(e));
    }

    tx.commit().await?;

    Ok(())
}

async fn check_websites_general(app_state: AppState) {
    match app_state {
        AppState::Postgres(p) => check_websites_postgres(p).await,
        AppState::Sqlite(s) => check_websites_sqlite(s).await,
    };
}

async fn check_websites_postgres(db: PgPool) {
    let mut interval = time::interval(Duration::from_secs(60));
    loop {
        interval.tick().await;

        info!("Starting Website Uptime check");
        let client = reqwest::Client::new();

        let mut res = sqlx::query_as::<_, Website>(SELECT_URL_ALIAS_WEBSITES_QUERY).fetch(&db);

        while let Some(website) = res.next().await {
            let website = website.unwrap();

            let response = client.get(website.url).send().await;

            let result = if let Ok(response) = response {
                sqlx::query(INSERT_INTO_LOGS_BY_ALIAS_RESPONSE_CODE_QUERY)
                    .bind(website.alias)
                    .bind(response.status().as_u16() as i16)
                    .bind("")
                    .execute(&db)
                    .await
            } else {
                // Website unreachable or Server has not internet access
                sqlx::query(INSERT_INTO_LOGS_BY_ALIAS_RESPONSE_CODE_QUERY)
                    .bind(website.alias)
                    .bind(-1 as i16)
                    .bind("Website unreachable or Server has not internet access!")
                    .execute(&db)
                    .await
            };
            if let Err(error) = result {
                error!("Error inserting log into db: {error}");
            }
        }
        info!("Finished Website Uptime check");
    }
}

async fn check_websites_sqlite(db: SqlitePool) {
    let mut interval = time::interval(Duration::from_secs(60));
    loop {
        interval.tick().await;

        info!("Starting Website Uptime check");
        let client = reqwest::Client::new();

        let mut res = sqlx::query_as::<_, Website>(SELECT_URL_ALIAS_WEBSITES_QUERY).fetch(&db);

        while let Some(website) = res.next().await {
            let website = website.unwrap();

            let response = client.get(website.url).send().await;

            let result = if let Ok(response) = response {
                sqlx::query(INSERT_INTO_LOGS_BY_ALIAS_RESPONSE_CODE_QUERY)
                    .bind(website.alias)
                    .bind(response.status().as_u16() as i16)
                    .bind("")
                    .execute(&db)
                    .await
            } else {
                // Website unreachable or Server has not internet access
                sqlx::query(INSERT_INTO_LOGS_BY_ALIAS_RESPONSE_CODE_QUERY)
                    .bind(website.alias)
                    .bind(-1 as i16)
                    .bind("Website unreachable or Server has no internet access!")
                    .execute(&db)
                    .await
            };
            if let Err(error) = result {
                error!("Error inserting log into db: {error}");
            }
        }
        info!("Finished Website Uptime check");
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn fill_data_gaps_returns_expected_amount_of_segments_24_hours_3600() {
        let result = fill_data_gaps(vec![], 24, SplitBy::Hour, 3600);
        assert_eq!(result.len(), 24);
    }

    #[test]
    fn fill_data_gaps_returns_expected_amount_of_segments_30_hours_86400() {
        let result = fill_data_gaps(vec![], 24, SplitBy::Day, 86400);
        assert_eq!(result.len(), 30);
    }

    #[test]
    fn fill_data_gaps_returns_expected_amount_of_segments_1_hours_3600() {
        let websitestat = WebsiteStats {
            time: Utc::now(),
            uptime_pct: Some(200),
        };
        let result = fill_data_gaps(vec![websitestat], 1, SplitBy::Hour, 3600);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn password_hashing_and_validation() {
        let password = "Test1234!";
        let hash_result = hash_password(&password);
        assert!(hash_result.is_ok(), "Password hashing should succeed");

        let hash = hash_result.unwrap();
        let user = User {
            username: String::new(),
            password_hash: hash.hash,
            salt: hash.salt,
        };
        let validation_result = verify_password(&user, &password);
        assert!(validation_result.is_ok(), "Password should match");
    }
}
