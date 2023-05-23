use chrono::{TimeZone, Utc};
use sea_orm_migration::prelude::{Table, *};

use crate::m20220828_125955_create_fishes_table::Fishes;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Seasons::Table)
                    .add_column(
                        ColumnDef::new(Seasons::Start)
                            .not_null()
                            .timestamp_with_time_zone(),
                    )
                    .add_column(ColumnDef::new(Seasons::End).timestamp_with_time_zone())
                    .to_owned(),
            )
            .await?;

        manager
            .exec_stmt(
                Query::insert()
                    .into_table(Seasons::Table)
                    .columns([Seasons::Id, Seasons::Name, Seasons::Start])
                    .values_panic(vec![
                        1.into(),
                        "Legacy".into(),
                        Utc.with_ymd_and_hms(2022, 8, 31, 12, 0, 0).unwrap().into(),
                    ])
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Catches::Table)
                    .add_column(
                        ColumnDef::new(Catches::SeasonId)
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    .add_foreign_key(
                        TableForeignKey::new()
                            .name("FK_catches_season_id")
                            .from_tbl(Catches::Table)
                            .from_col(Catches::SeasonId)
                            .to_tbl(Seasons::Table)
                            .to_col(Seasons::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(FishesSeasons::Table)
                    // .col(
                    //     ColumnDef::new(FishesSeasons::Id)
                    //         .integer()
                    //         .not_null()
                    //         .primary_key()
                    //         .auto_increment(),
                    // )
                    .col(
                        ColumnDef::new(FishesSeasons::FishId)
                            .integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(FishesSeasons::SeasonId)
                            .integer()
                            .not_null()
                            .primary_key(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("FK_fishes_seasons_fish_id")
                            .from(FishesSeasons::Table, FishesSeasons::FishId)
                            .to(Fishes::Table, Fishes::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("FK_fishes_seasons_season_id")
                            .from(FishesSeasons::Table, FishesSeasons::SeasonId)
                            .to(Seasons::Table, Seasons::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .exec_stmt(
                Query::insert()
                    .into_table(FishesSeasons::Table)
                    .columns([FishesSeasons::FishId, FishesSeasons::SeasonId])
                    .select_from(
                        Query::select()
                            .column(Fishes::Id)
                            .expr(Expr::val(1))
                            .from(Fishes::Table)
                            .to_owned(),
                    )
                    .unwrap()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Catches::Table)
                    .drop_column(Catches::SeasonId)
                    .drop_foreign_key(Alias::new("FK_catches_season_id"))
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Seasons::Table)
                    .drop_column(Seasons::Start)
                    .drop_column(Seasons::End)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_table(Table::drop().table(FishesSeasons::Table).to_owned())
            .await?;

        manager
            .exec_stmt(
                Query::delete()
                    .from_table(Seasons::Table)
                    .and_where(Expr::col(Seasons::Id).eq(1))
                    .to_owned(),
            )
            .await
    }
}

/// Learn more at https://docs.rs/sea-query#iden
#[derive(Iden)]
pub enum Catches {
    Table,
    SeasonId,
}

#[derive(Iden)]
pub enum Seasons {
    Table,
    Id,
    Name,
    Start,
    End,
}

#[derive(Iden)]
pub enum FishesSeasons {
    Table,
    Id,
    FishId,
    SeasonId,
}
