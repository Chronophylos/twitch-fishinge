#![forbid(unsafe_code)]

#[allow(clippy::derive_partial_eq_without_eq)]
pub mod entities;

use std::{env, time::Duration};

use log::debug;
use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectOptions, Database, DatabaseConnection};

const DATABASE_URL: &str = "mysql://postgres:postgres@localhost:3306";

#[derive(Debug, thiserror::Error)]
#[error("Could not close database connection")]
pub enum Error {
    #[error("Could not connect to database")]
    Connect(#[source] sea_orm::DbErr),

    #[error("Could not migrate database")]
    Migrate(#[source] sea_orm::DbErr),
}

pub async fn connection() -> Result<DatabaseConnection, Error> {
    debug!("Opening database connection");

    let mut opt = ConnectOptions::new(
        env::var("DATABASE_URL")
            .as_deref()
            .unwrap_or(DATABASE_URL)
            .to_owned(),
    );
    opt.connect_timeout(Duration::from_secs(5))
        .sqlx_logging_level(log::LevelFilter::Debug);

    let db = Database::connect(opt).await.map_err(Error::Connect)?;

    Ok(db)
}

pub async fn migrate(db: &DatabaseConnection) -> Result<(), Error> {
    Migrator::up(db, None).await.map_err(Error::Migrate)?;
    Ok(())
}
