use axum::{Router, response::Html, routing::get};
use clap::Parser;
use serde::Deserialize;
use sqlx::{PgPool, SqlitePool, migrate::Migrator};
use tokio::time::{Duration, sleep};
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

    async fn fetch()
}

const SQLITE_CONNECTION_STRING: &'static str = "sqlite://uptime_ferris.db?mode=rwc";

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let app_state = AppState::from(args).await;
    // carry out migrations
    let _ = &app_state.migrate_db().await;
    // build our application with a route
    let app = Router::new().route("/", get(handler).with_state(app_state));

    let calling_myself = tokio::spawn(calling_myself());
    // run it
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();
    println!("listening on {}", listener.local_addr().unwrap());
    let web_server = axum::serve(listener, app);
    let _result = tokio::join!(calling_myself, web_server);
}

async fn handler() -> Html<&'static str> {
    Html("<h1>Hello, World!</h1>")
}

async fn calling_myself() {
    sleep(Duration::from_millis(5000)).await;
    let client = reqwest::Client::new();
    let _respone = client.get("http://127.0.0.1:3000/").send().await.unwrap();
    println!("called myself");
}

async fn check_website(appState: AppState) {
    let client = reqwest::Client::new();

    let query = sqlx::query_as::<_, Website>("SELECT url, alias FROM Websites");
    let mut res = match appState {
        AppState::Postgres(p) => 
        AppState::Sqlite(s) =>
    }
     
}
