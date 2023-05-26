use sea_orm_migration::prelude::*;

const QUERY_BUILDER: PostgresQueryBuilder = PostgresQueryBuilder;

#[derive(DeriveMigrationName)]
pub struct Migration;

macro_rules! debug_query {
    ($query:expr) => {
        match $query {
            val => {
                eprintln!(
                    "[{}:{}]: {}",
                    file!(),
                    line!(),
                    val.to_string(QUERY_BUILDER)
                );
                val
            }
        }
    };
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // create set table
        manager
            .create_table(debug_query!(Table::create()
                .table(Bundle::Table)
                .col(
                    ColumnDef::new(Bundle::Id)
                        .primary_key()
                        .auto_increment()
                        .not_null()
                        .integer(),
                )
                .take()))
            .await?;

        // rename fishesseason to fishset
        manager
            .rename_table(debug_query!(Table::rename()
                .table(FishesSeasons::Table, FishBundle::Table)
                .take()))
            .await?;

        // rename fishset.season_id to set_id and make it a foreign key
        manager
            .alter_table(debug_query!(Table::alter()
                .table(FishBundle::Table)
                .rename_column(FishBundle::SeasonId, FishBundle::BundleId)
                .drop_foreign_key(Alias::new("FK_fishes_seasons_season_id"))
                .add_foreign_key(
                    TableForeignKey::new()
                        .name("FK_fishset_set_id")
                        .from_tbl(FishBundle::Table)
                        .from_col(FishBundle::BundleId)
                        .to_tbl(Bundle::Table)
                        .to_col(Bundle::Id),
                )
                .take()))
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // revert rename fishset.season_id to set_id and make it a foreign key
        manager
            .alter_table(
                Table::alter()
                    .table(FishBundle::Table)
                    .rename_column(FishBundle::BundleId, FishBundle::SeasonId)
                    .drop_foreign_key(Alias::new("FK_fishset_set_id"))
                    .add_foreign_key(
                        TableForeignKey::new()
                            .name("FK_fishes_seasons_season_id")
                            .from_tbl(FishBundle::Table)
                            .from_col(FishBundle::SeasonId)
                            .to_tbl(Seasons::Table)
                            .to_col(Seasons::Id),
                    )
                    .take(),
            )
            .await?;

        // revert rename fishesseason to fishset
        manager
            .rename_table(
                Table::rename()
                    .table(FishBundle::Table, FishesSeasons::Table)
                    .to_owned(),
            )
            .await?;

        // revert create set table
        manager
            .drop_table(Table::drop().table(Bundle::Table).take())
            .await?;

        Ok(())
    }
}

/// Learn more at https://docs.rs/sea-query#iden
#[derive(Iden)]
enum FishesSeasons {
    Table,
}

#[derive(Iden)]
enum FishBundle {
    Table,
    SeasonId,
    BundleId,
}

#[derive(Iden)]
enum Bundle {
    Table,
    Id,
}

#[derive(Iden)]
enum Seasons {
    Table,
    Id,
}
