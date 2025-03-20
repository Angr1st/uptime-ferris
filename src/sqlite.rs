use sqlx::{SqlitePool, migrate::Migrator};

pub async fn migrate_sqlite(pool: &SqlitePool) {
    let migrator = Migrator::new(std::path::Path::new("./migrations_sq"))
        .await
        .expect("Migrations folder couldn't be found");
    migrator
        .run(pool)
        .await
        .expect("Sqlite migration(s) failed");
}
