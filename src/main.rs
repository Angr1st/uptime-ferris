use axum::{Router, response::Html, routing::get};
use clap::Parser;
use sqlx::{PgPool, SqlitePool};
use tokio::time::{Duration, sleep};

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
}

const SQLITE_CONNECTION_STRING: &'static str = "sqlite:uptime_ferris.db";

impl AppState {
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

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let app_state = AppState::from(args).await;
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
