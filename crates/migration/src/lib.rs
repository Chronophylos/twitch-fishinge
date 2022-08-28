pub use sea_orm_migration::prelude::*;

mod m20220828_125955_create_fishes_table;
mod m20220828_131908_create_users_table;
mod m20220828_132240_create_catches_table;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20220828_125955_create_fishes_table::Migration),
            Box::new(m20220828_131908_create_users_table::Migration),
            Box::new(m20220828_132240_create_catches_table::Migration),
        ]
    }
}
