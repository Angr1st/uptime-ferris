use clap::Parser;

/// Configure either Postgres or Sqlite connection string
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    /// Postgres Db Connection String
    #[arg(short, long, env, default_value = None)]
    pub(crate) pg: Option<String>,

    /// Sqlite Db
    #[arg(short, long, env, default_value_t = true)]
    pub(crate) sqlite: bool,
}
