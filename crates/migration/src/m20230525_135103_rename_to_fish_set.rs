use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // create bundle table
        manager
            .create_table(
                Table::create()
                    .table(Bundle::Table)
                    .col(
                        ColumnDef::new(Bundle::Id)
                            .primary_key()
                            .auto_increment()
                            .not_null()
                            .integer(),
                    )
                    .take(),
            )
            .await?;

        // create empty bundle
        manager
            .exec_stmt(
                Query::insert()
                    .into_table(Bundle::Table)
                    .columns([Bundle::Id])
                    .values_panic(vec![0.into()])
                    .to_owned(),
            )
            .await?;

        // rename fishesseason to fishset
        manager
            .rename_table(
                Table::rename()
                    .table(FishesSeasons::Table, FishBundle::Table)
                    .take(),
            )
            .await?;

        // rename fishset.season_id to set_id
        manager
            .alter_table(
                Table::alter()
                    .table(FishBundle::Table)
                    .rename_column(FishBundle::SeasonId, FishBundle::BundleId)
                    .take(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(FishBundle::Table)
                    .drop_foreign_key(Alias::new("FK_fishes_seasons_season_id"))
                    .add_foreign_key(
                        TableForeignKey::new()
                            .name("FK_fishbundle_bundle_id")
                            .from_tbl(FishBundle::Table)
                            .from_col(FishBundle::BundleId)
                            .to_tbl(Bundle::Table)
                            .to_col(Bundle::Id),
                    )
                    .take(),
            )
            .await?;

        // add bundle_id column to season
        manager
            .alter_table(
                Table::alter()
                    .table(Seasons::Table)
                    .add_column(
                        ColumnDef::new(Seasons::BundleId)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .add_foreign_key(
                        TableForeignKey::new()
                            .name("FK_seasons_bundle_id")
                            .from_tbl(Seasons::Table)
                            .from_col(Seasons::BundleId)
                            .to_tbl(Bundle::Table)
                            .to_col(Bundle::Id),
                    )
                    .take(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // revert add bundle_id column to season
        manager
            .alter_table(
                Table::alter()
                    .table(Seasons::Table)
                    .drop_foreign_key(Alias::new("FK_seasons_bundle_id"))
                    .drop_column(Seasons::BundleId)
                    .take(),
            )
            .await?;

        // revert rename fishset.season_id to set_id and make it a foreign key
        manager
            .alter_table(
                Table::alter()
                    .table(FishBundle::Table)
                    .rename_column(FishBundle::BundleId, FishBundle::SeasonId)
                    .take(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(FishBundle::Table)
                    .drop_foreign_key(Alias::new("FK_fishbundle_bundle_id"))
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

        // revert rename fishesseason to fishbundle
        manager
            .rename_table(
                Table::rename()
                    .table(FishBundle::Table, FishesSeasons::Table)
                    .to_owned(),
            )
            .await?;

        // revert create empty bundle
        manager
            .exec_stmt(
                Query::delete()
                    .from_table(Bundle::Table)
                    .and_where(Expr::col(Bundle::Id).eq(0))
                    .to_owned(),
            )
            .await?;

        // revert create bundle table
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
    BundleId,
}
