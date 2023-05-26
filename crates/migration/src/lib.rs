pub use sea_orm_migration::prelude::*;

mod m20220828_125955_create_fishes_table;
mod m20220828_131908_create_users_table;
mod m20220828_132240_create_catches_table;
mod m20220828_135214_create_messages_table;
mod m20220828_143222_create_seasons_table;
mod m20220828_143304_create_seasons_data_table;
mod m20220829_150037_create_accounts_table;
mod m20230426_115812_integrate_seasons;
mod m20230525_135103_rename_to_fish_set;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20220828_125955_create_fishes_table::Migration),
            Box::new(m20220828_131908_create_users_table::Migration),
            Box::new(m20220828_132240_create_catches_table::Migration),
            Box::new(m20220828_135214_create_messages_table::Migration),
            Box::new(m20220828_143222_create_seasons_table::Migration),
            Box::new(m20220828_143304_create_seasons_data_table::Migration),
            Box::new(m20220829_150037_create_accounts_table::Migration),
            Box::new(m20230426_115812_integrate_seasons::Migration),
            Box::new(m20230525_135103_rename_to_fish_set::Migration),
        ]
    }
}
