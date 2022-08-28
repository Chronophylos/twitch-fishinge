use sea_orm_migration::prelude::*;

use super::{
    m20220828_125955_create_fishes_table::Fishes, m20220828_131908_create_users_table::Users,
};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Catches::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Catches::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Catches::UserId).integer().not_null())
                    .col(ColumnDef::new(Catches::FishId).integer().not_null())
                    .col(ColumnDef::new(Catches::Weight).float())
                    .col(
                        ColumnDef::new(Catches::CaughtAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Catches::Value).float().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-catch-user_id")
                            .from(Catches::Table, Catches::UserId)
                            .to(Users::Table, Users::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-catch-fish_id")
                            .from(Catches::Table, Catches::FishId)
                            .to(Fishes::Table, Fishes::Id),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Catches::Table).to_owned())
            .await
    }
}

/// Learn more at https://docs.rs/sea-query#iden
#[derive(Iden)]
pub enum Catches {
    Table,
    Id,
    CaughtAt,
    UserId,
    FishId,
    Weight,
    Value,
}
