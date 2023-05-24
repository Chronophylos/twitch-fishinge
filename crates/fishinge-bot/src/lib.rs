#![forbid(unsafe_code)]

use std::{fmt::Display, ops::Range, sync::RwLock};

use async_trait::async_trait;
use chrono::{DateTime, Datelike, FixedOffset, Offset, TimeZone, Utc};
use database::entities::{accounts, fishes, prelude::*, seasons};
use eyre::{eyre, Result, WrapErr};
use rand::Rng;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, FromQueryResult,
    Linked, ModelTrait, QueryFilter, QueryOrder, QuerySelect, RelationTrait,
};
use twitch_irc::login::{TokenStorage, UserAccessToken};

pub static FISH_POPULATION: RwLock<i32> = RwLock::new(0);

#[derive(Debug, Clone)]
pub struct Fish {
    pub id: i32,
    pub name: String,
    pub count: u32,
    pub base_value: i32,
    pub weight_range: Option<Range<f32>>,
}

impl Fish {
    pub fn catch(&self) -> Catch {
        let mut rng = rand::thread_rng();

        let weight = self
            .weight_range
            .clone()
            .map(|weight| rng.gen_range(weight));

        Catch::new(self, weight)
    }
}

impl From<database::entities::fishes::Model> for Fish {
    fn from(fish: database::entities::fishes::Model) -> Self {
        Self {
            id: fish.id,
            name: fish.name,
            count: fish.count as u32,
            base_value: fish.base_value as i32,
            weight_range: if fish.min_weight > f32::EPSILON && fish.max_weight > f32::EPSILON {
                Some(fish.min_weight..fish.max_weight)
            } else {
                None
            },
        }
    }
}

impl Display for Fish {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({:.1}%)",
            self.name,
            self.count as f32 / *FISH_POPULATION.read().unwrap() as f32 * 100.0
        )?;

        if let Some(weight) = &self.weight_range {
            write!(f, " ({:.1}kg - {:.1}kg)", weight.start, weight.end)?;
        }

        Ok(())
    }
}

pub async fn get_active_season(db: &DatabaseConnection) -> Result<seasons::Model> {
    let season = Seasons::find()
        .filter(seasons::Column::Start.lt(chrono::Utc::now()))
        .filter(
            seasons::Column::End
                .gt(chrono::Utc::now())
                .or(seasons::Column::End.is_null()),
        )
        .one(db)
        .await
        .wrap_err("Could not fetch seasons")?;

    if let Some(season) = season {
        Ok(season)
    } else {
        Err(eyre!("No active season found"))
    }
}

pub async fn has_next_season(db: &DatabaseConnection) -> Result<bool> {
    let season = Seasons::find()
        .filter(seasons::Column::Start.gt(chrono::Utc::now()))
        .one(db)
        .await
        .wrap_err("Could not fetch seasons")?;

    Ok(season.is_some())
}

fn find_subsequent_quarter(start: DateTime<FixedOffset>) -> (i32, Quarter) {
    match start.month() {
        1 | 2 | 3 => (start.year(), Quarter::Second),
        4 | 5 | 6 => (start.year(), Quarter::Third),
        7 | 8 | 9 => (start.year(), Quarter::Fourth),
        10 | 11 | 12 => (start.year() + 1, Quarter::First),
        _ => unreachable!(),
    }
}

enum Quarter {
    First,
    Second,
    Third,
    Fourth,
}

async fn create_season(
    db: &DatabaseConnection,
    name: String,
    start: DateTime<FixedOffset>,
    end: DateTime<FixedOffset>,
) -> Result<()> {
    Seasons::insert(seasons::ActiveModel {
        name: ActiveValue::set(name),
        start: ActiveValue::set(start),
        end: ActiveValue::set(Some(end)),
        ..Default::default()
    })
    .exec(db)
    .await?;

    Ok(())
}

pub async fn create_next_season(db: &DatabaseConnection) -> Result<()> {
    let Some(latest_season) = Seasons::find()
        .order_by_desc(seasons::Column::Start)
        .one(db)
        .await? else {
        return Err(eyre!("No season found"))
    };

    let (year, quarter) = find_subsequent_quarter(latest_season.start);
    let (name, start) = match quarter {
        Quarter::First => (
            format!("Spring {year}"),
            Utc.with_ymd_and_hms(year, 1, 1, 12, 0, 0)
                .unwrap()
                .with_timezone(&Utc.fix()),
        ),
        Quarter::Second => (
            format!("Summer {year}"),
            Utc.with_ymd_and_hms(year, 4, 1, 12, 0, 0)
                .unwrap()
                .with_timezone(&Utc.fix()),
        ),
        Quarter::Third => (
            format!("Autumn {year}"),
            Utc.with_ymd_and_hms(year, 7, 1, 12, 0, 0)
                .unwrap()
                .with_timezone(&Utc.fix()),
        ),
        Quarter::Fourth => (
            format!("Winter {year}"),
            Utc.with_ymd_and_hms(year, 10, 1, 12, 0, 0)
                .unwrap()
                .with_timezone(&Utc.fix()),
        ),
    };

    create_season(db, name, start, start + chrono::Duration::days(90)).await?;

    Ok(())
}

pub async fn get_fishes(db: &DatabaseConnection, season: &seasons::Model) -> Result<Vec<Fish>> {
    let fishes = season.find_related(Fishes).all(db).await?;

    let population = fishes.iter().map(|fish| fish.count).sum();

    *FISH_POPULATION.write().unwrap() = population;

    Ok(fishes.into_iter().map(Fish::from).collect())
}

#[derive(Debug, Clone)]
pub struct Catch {
    pub fish_name: String,
    pub weight: Option<f32>,
    pub value: f32,
}

impl Catch {
    pub fn new(fish: &Fish, weight: Option<f32>) -> Self {
        let multiplier = fish
            .weight_range
            .as_ref()
            .and_then(|range| {
                weight.map(|weight| (weight - range.start) / (range.end - range.start))
            })
            .map_or(1.0, |x| (x * 1.36 - 0.48).powi(3) + 1.01 + x * 0.11);

        Self {
            fish_name: fish.name.clone(),
            weight,
            value: fish.base_value as f32 * multiplier,
        }
    }
}

impl Display for Catch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.fish_name)?;
        if let Some(weight) = self.weight {
            write!(f, " ({:.1}kg)", weight)?;
        }
        if self.value.abs() > f32::EPSILON {
            write!(f, " worth ${:.2}", self.value)?;
        } else {
            write!(f, " worth nothing")?;
        }

        Ok(())
    }
}

#[derive(Debug)]
pub struct Account {
    id: i32,
    db: DatabaseConnection,
}

impl Account {
    pub async fn new(db: DatabaseConnection, username: &str) -> Result<Self> {
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
            .await?
            .ok_or_else(|| eyre!("account `{username}` not found in database"))?
            .id;

        Ok(Self { id, db })
    }
}

#[async_trait]
impl TokenStorage for Account {
    type LoadError = eyre::Error;
    type UpdateError = eyre::Error;

    async fn load_token(&mut self) -> Result<UserAccessToken, Self::LoadError> {
        let account = Accounts::find_by_id(self.id)
            .one(&self.db)
            .await
            .wrap_err("Could not query account")?
            .ok_or_else(|| eyre::eyre!("Account not found"))?;

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

        account
            .update(&self.db)
            .await
            .wrap_err("Could not update account")?;

        Ok(())
    }
}
