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
use tokio::time::{self, Duration};
use validator::Validate;

/// Configure either Postgres or Sqlite connection string
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Postgres Db Connection String
    #[arg(short, long, env, default_value = None)]
    pg: Option<String>,

    /// Sqlite Db
    #[arg(short, long, env, default_value_t = true)]
    sqlite: bool,
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

#[derive(Clone)]
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
            Self::Sqlite(s) => Self::migrate_sqlite(s).await,
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

    async fn migrate_sqlite(pool: &SqlitePool) {
        let migrator = Migrator::new(std::path::Path::new("./migrations_sq"))
            .await
            .expect("Migrations folder couldn't be found");
        migrator.run(pool).await.expect("Sqlite migration failed");
    }

    async fn from(item: Args) -> Self {
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
    let args = Args::parse();
    let app_state = AppState::from(args).await;
    // carry out migrations
    let _ = &app_state.migrate_db().await;
    let cloned_state = app_state.clone();
    //Check the website status
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
        .with_state(app_state);

    // run it
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();
    println!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();
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
            let _ = sqlx::query("INSERT INTO Websites (url, alias) VALUES ($1,$2)")
                .bind(new_website.url)
                .bind(new_website.alias)
                .execute(&p)
                .await
                .unwrap();
        }
        AppState::Sqlite(s) => {
            let _ = sqlx::query("INSERT INTO Websites (url, alias) VALUES ($1,$2)")
                .bind(new_website.url)
                .bind(new_website.alias)
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
            sqlx::query_as::<_, Website>("SELECT url, alias FROM Websites")
                .fetch_all(p)
                .await?
        }
        AppState::Sqlite(ref s) => {
            sqlx::query_as::<_, Website>("SELECT url, alias FROM Websites")
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
            sqlx::query_as::<_,WebsiteStats>(
                r#"
                SELECT date_trunc('hour', created_at) as time,
                CAST(COUNT(case when status = 200 then 1 end) * 100 / COUNT(*) as int2) as uptime_pct
                FROM Logs
                LEFT JOIN Websites on Websites.id = Logs.website_id
                WHERE Websites.alias = $1
                GROUP BY time
                ORDER BY time asc
                LIMIT 24
                "#
            )
            .bind(alias)
            .fetch_all(p).await?
        },
        AppState::Sqlite(s) => {
            sqlx::query_as::<_,WebsiteStats>(
                r#"
                SELECT strftime('%Y-%m-%d %H:00:00', created_at) as time,
                CAST(COUNT(CASE WHEN status = 200 THEN 1 END) * 100 / COUNT(*) AS INTEGER) as uptime_pct
                FROM Logs
                LEFT JOIN Websites ON Websites.id = Logs.website_id
                WHERE Websites.alias = $1
                GROUP BY time
                ORDER BY time ASC
                LIMIT 24
                "#
            )
            .bind(alias)
            .fetch_all(s).await?
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
            sqlx::query_as::<_, WebsiteStats>(
                r#"
                Select date_trunc('day', created_at) as time,
                CAST(COUNT(case when status = 200 then 1) * 100 / COUNT(*) AS int2) AS uptime_pct
                FROM Logs
                LEFT JOIN Websites ON Websites.id = Logs.website_id
                WHERE Websites.alias = $1
                GROUP BY time
                ORDER BY time asc
                LIMIT 30
            "#,
            )
            .bind(alias)
            .fetch_all(p)
            .await?
        }
        AppState::Sqlite(s) => {
            sqlx::query_as::<_, WebsiteStats>(
                r#"
                SELECT strftime('%Y-%m-%d 00:00:00', created_at) as time,
                CAST(COUNT(CASE WHEN status = 200 THEN 1 END) * 100 / COUNT(*) AS INTEGER) as uptime_pct
                FROM Logs
                LEFT JOIN Websites ON Websites.id = Logs.website_id
                WHERE Websites.alias = $1
                GROUP BY time
                ORDER BY time ASC
                LIMIT 30
            "#,
            )
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
    let website = match state {
        AppState::Postgres(ref p) => {
            sqlx::query_as::<_, Website>("SELECT url, alias FROM Websites WHERE alias = $1 LIMIT 1")
                .bind(&alias)
                .fetch_one(p)
                .await?
        }
        AppState::Sqlite(ref s) => {
            sqlx::query_as::<_, Website>("SELECT url, alias FROM Websites WHERE alias = $1 LIMIT 1")
                .bind(&alias)
                .fetch_one(s)
                .await?
        }
    };

    let last_24_hours_data = get_daily_stats(&website.alias, &state).await?;
    let monthly_data = get_monthly_stats(&website.alias, &state).await?;

    let incidents = match state {
        AppState::Postgres(p) => {
            sqlx::query_as::<_, Incident>(
                "
            SELECT Logs.created_at as time,
            Logs.status from Logs
            LEFT JOIN Websites on Websites.id = Logs.website_id
            where Websites.Alias = $1 and Logs.status <> 200
            ",
            )
            .bind(&alias)
            .fetch_all(&p)
            .await?
        }
        AppState::Sqlite(s) => {
            sqlx::query_as::<_, Incident>(
                "
            SELECT Logs.created_at as time,
            Logs.status from Logs
            LEFT JOIN Websites on Websites.id = Logs.website_id
            where Websites.Alias = $1 and Logs.status <> 200
            ",
            )
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
    if let Err(e) = sqlx::query(
        "DELETE FROM Logs
        LEFT JOIN Websites ON Websites.id = Logs.website_id
        WHERE Websites.alias = $1",
    )
    .bind(alias)
    .execute(&mut *tx)
    .await
    {
        tx.rollback().await?;
        return Err(ApiError::SQL(e));
    };

    if let Err(e) = sqlx::query("DELETE FROM Websites WHERE alias = $1")
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
    if let Err(e) = sqlx::query(
        "DELETE FROM Logs
        LEFT JOIN Websites ON Websites.id = Logs.website_id
        WHERE Websites.alias = $1",
    )
    .bind(alias)
    .execute(&mut *tx)
    .await
    {
        tx.rollback().await?;
        return Err(ApiError::SQL(e));
    };

    if let Err(e) = sqlx::query("DELETE FROM Websites WHERE alias = $1")
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

        let mut res = sqlx::query_as::<_, Website>("SELECT url, alias FROM Websites").fetch(&db);

        while let Some(website) = res.next().await {
            let website = website.unwrap();

            let response = client.get(website.url).send().await.unwrap();

            sqlx::query(
                "INSERT INTO Logs (website_id, status)
                VALUES
                ((SELECT id FROM Websites WHERE alias = $1), $2)",
            )
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

        let client = reqwest::Client::new();

        let mut res = sqlx::query_as::<_, Website>("SELECT url, alias FROM Websites").fetch(&db);

        while let Some(website) = res.next().await {
            let website = website.unwrap();

            let response = client.get(website.url).send().await.unwrap();

            sqlx::query(
                "INSERT INTO Logs (website_id, status)
                VALUES
                ((SELECT id FROM Websites WHERE alias = $1), $2)",
            )
            .bind(website.alias)
            .bind(response.status().as_u16() as i16)
            .execute(&db)
            .await
            .unwrap();
        }
    }
}
