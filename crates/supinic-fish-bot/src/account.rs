use async_trait::async_trait;
use database::entities::{accounts, prelude::Accounts};
use miette::Diagnostic;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, DbErr, EntityTrait,
    FromQueryResult, QueryFilter, QuerySelect,
};
use twitch_irc::login::{TokenStorage, UserAccessToken};

#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum Error {
    #[error("could not query account")]
    #[diagnostic(code(account::query_failed))]
    QueryFailed(#[source] DbErr),

    #[error("account not found")]
    #[diagnostic(code(account::not_found))]
    NotFound,

    #[error("could not update token")]
    #[diagnostic(code(account::update_token))]
    UpdateToken(#[source] DbErr),
}

#[derive(Debug)]
pub struct Account {
    id: i32,
    db: DatabaseConnection,
}

impl Account {
    pub async fn new(db: DatabaseConnection, username: &str) -> Result<Self, Error> {
        #[derive(FromQueryResult)]
        struct AccountId {
            id: i32,
        }

        let id = Accounts::find()
            .filter(accounts::Column::Username.eq(username))
            .select_only()
            .column(accounts::Column::Id)
            .into_model::<AccountId>()
            .one(&db)
            .await
            .map_err(Error::QueryFailed)?
            .ok_or(Error::NotFound)?
            .id;

        Ok(Self { id, db })
    }
}

#[async_trait]
impl TokenStorage for Account {
    type LoadError = Error;
    type UpdateError = Error;

    async fn load_token(&mut self) -> Result<UserAccessToken, Self::LoadError> {
        let account = Accounts::find_by_id(self.id)
            .one(&self.db)
            .await
            .map_err(Error::QueryFailed)?
            .ok_or(Error::NotFound)?;

        Ok(UserAccessToken {
            access_token: account.access_token,
            refresh_token: account.refresh_token,
            created_at: account.created_at.into(),
            expires_at: account.expires_at.map(Into::into),
        })
    }

    async fn update_token(&mut self, token: &UserAccessToken) -> Result<(), Self::UpdateError> {
        let account = accounts::ActiveModel {
            id: ActiveValue::unchanged(self.id),
            access_token: ActiveValue::set(token.access_token.clone()),
            refresh_token: ActiveValue::set(token.refresh_token.clone()),
            created_at: ActiveValue::set(token.created_at.into()),
            expires_at: ActiveValue::set(token.expires_at.map(Into::into)),
            ..Default::default()
        };

        account.update(&self.db).await.map_err(Error::UpdateToken)?;

        Ok(())
    }
}
