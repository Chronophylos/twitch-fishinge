#![forbid(unsafe_code)]

pub mod models;

use std::{env, str::FromStr};

use log::debug;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode},
    ConnectOptions, SqliteConnection,
};

#[derive(Debug, thiserror::Error)]
#[error("Could not close database connection")]
pub struct OpenDatabaseError(#[from] sqlx::Error);

pub async fn db_conn() -> Result<SqliteConnection, OpenDatabaseError> {
    debug!("Opening database connection");
    let conn = SqliteConnectOptions::from_str(
        env::var("DATABASE_URL")
            .as_deref()
            .unwrap_or("sqlite://fish.db"),
    )?
    .journal_mode(SqliteJournalMode::Wal)
    .create_if_missing(true)
    .connect()
    .await?;

    Ok(conn)
}
