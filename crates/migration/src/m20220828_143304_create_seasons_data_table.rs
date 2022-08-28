use sea_orm_migration::prelude::*;

use crate::{
    m20220828_131908_create_users_table::Users, m20220828_143222_create_seasons_table::Seasons,
};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(SeasonData::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SeasonData::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(SeasonData::SeasonId).integer().not_null())
                    .col(ColumnDef::new(SeasonData::UserId).integer().not_null())
                    .col(ColumnDef::new(SeasonData::Score).float().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-season_data-user_id")
                            .from(SeasonData::Table, SeasonData::UserId)
                            .to(Users::Table, Users::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-season_data-season_id")
                            .from(SeasonData::Table, SeasonData::SeasonId)
                            .to(Seasons::Table, Seasons::Id),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(SeasonData::Table).to_owned())
            .await
    }
}

/// Learn more at https://docs.rs/sea-query#iden
#[derive(Iden)]
enum SeasonData {
    Table,
    Id,
    SeasonId,
    UserId,
    Score,
}
