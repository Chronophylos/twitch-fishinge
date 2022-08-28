#![forbid(unsafe_code)]

pub mod entities;

use std::env;

use log::debug;
use migration::{Migrator, MigratorTrait};
use sea_orm::{Database, DatabaseConnection};

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

    let db = Database::connect(env::var("DATABASE_URL").as_deref().unwrap_or(DATABASE_URL))
        .await
        .map_err(Error::Connect)?;

    Ok(db)
}

pub async fn migrate(db: &DatabaseConnection) -> Result<(), Error> {
    Migrator::refresh(db).await.map_err(Error::Migrate)?;
    Ok(())
}
