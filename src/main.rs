use crate::shared_queries::*;
use argument_parsing::Args;
use askama::Template;
use askama_axum::IntoResponse as AskamaIntoResponse;
use axum::{
    Form, Router,
    extract::{Path, State},
    response::{IntoResponse as AxumIntoResponse, Redirect, Response},
    routing::{get, post},
};
use chrono::{DateTime, Timelike, Utc};
use clap::Parser;
use futures_util::StreamExt;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, SqlitePool, migrate::Migrator};
use tokio::{
    signal,
    time::{self, Duration},
};
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use validator::Validate;

mod argument_parsing;
mod postgres_queries;
mod shared_queries;
mod sqlite;
mod sqlite_queries;

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

#[derive(Serialize, sqlx::FromRow, Template)]
#[template(path = "index.html")]
struct WebsiteLogs {
    logs: Vec<WebsiteInfo>,
}

#[derive(Serialize, sqlx::FromRow, Template)]
#[template(path = "single_website.html")]
struct SingleWebsiteLog {
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
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        Self::SQL(e)
    }
}

impl AxumIntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::SQL(e) => AxumIntoResponse::into_response((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("SQL Error: {e}"),
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
        .route("/", get(get_websites))
        .route("/websites", post(create_website))
        .route(
            "/websites/:alias",
            get(get_website_by_alias).delete(delete_website),
        )
        .route("/styles.css", get(styles))
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

async fn styles() -> impl AxumIntoResponse {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/css")
        .body(include_str!("../templates/styles.css").to_owned())
        .unwrap()
}

async fn create_website(
    State(state): State<AppState>,
    Form(new_website): Form<Website>,
) -> Result<impl AxumIntoResponse, impl AxumIntoResponse> {
    if new_website.validate().is_err() {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Validation Error: is your website a reachable URL?",
        ));
    }

    match state {
        AppState::Postgres(p) => {
            let _ = sqlx::query(INSERT_INTO_WEBSITES_QUERY)
                .bind(new_website.url)
                .bind(new_website.alias)
                .execute(&p)
                .await
                .unwrap();
        }
        AppState::Sqlite(s) => {
            let _ = sqlx::query(INSERT_INTO_WEBSITES_QUERY)
                .bind(new_website.url)
                .bind(new_website.alias)
                .bind(Utc::now())
                .execute(&s)
                .await
                .unwrap();
        }
    }

    Ok(Redirect::to("/"))
}

#[axum::debug_handler]
async fn get_websites(State(state): State<AppState>) -> Result<impl AskamaIntoResponse, ApiError> {
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

    Ok(WebsiteLogs { logs })
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
    // If the length of data is not as long as the number of required splits (24)
    // then we fill in the gaps
    if (data.len() as i32) < splits {
        for i in 1..24 {
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

    Ok(SingleWebsiteLog {
        log,
        incidents,
        monthly_data,
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

        let client = reqwest::Client::new();

        let mut res = sqlx::query_as::<_, Website>(SELECT_URL_ALIAS_WEBSITES_QUERY).fetch(&db);

        while let Some(website) = res.next().await {
            let website = website.unwrap();

            let response = client.get(website.url).send().await.unwrap();

            sqlx::query(INSERT_INTO_LOGS_BY_ALIAS_RESPONSE_CODE_QUERY)
                .bind(website.alias)
                .bind(response.status().as_u16() as i16)
                .execute(&db)
                .await
                .unwrap();
        }
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

            let response = client.get(website.url).send().await.unwrap();

            sqlx::query(INSERT_INTO_LOGS_BY_ALIAS_RESPONSE_CODE_QUERY)
                .bind(website.alias)
                .bind(response.status().as_u16() as i16)
                .execute(&db)
                .await
                .unwrap();
        }
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
