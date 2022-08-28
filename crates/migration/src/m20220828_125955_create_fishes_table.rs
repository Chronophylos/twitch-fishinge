use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Fishes::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Fishes::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Fishes::Name).string().not_null())
                    .col(ColumnDef::new(Fishes::HtmlName).string().not_null())
                    .col(
                        ColumnDef::new(Fishes::Count)
                            .integer()
                            .unsigned()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Fishes::BaseValue).float().not_null())
                    .col(ColumnDef::new(Fishes::MaxWeight).float().not_null())
                    .col(ColumnDef::new(Fishes::MinWeight).float().not_null())
                    .col(ColumnDef::new(Fishes::IsTrash).boolean().not_null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Fishes::Table).to_owned())
            .await
    }
}

/// Learn more at https://docs.rs/sea-query#iden
#[derive(Iden)]
pub enum Fishes {
    Table,
    Id,
    HtmlName,
    Name,
    Count,
    BaseValue,
    MaxWeight,
    MinWeight,
    IsTrash,
}
