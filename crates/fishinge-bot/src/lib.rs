#![forbid(unsafe_code)]

use std::{fmt::Display, ops::Range, sync::RwLock};

use async_trait::async_trait;
use chrono::{DateTime, Datelike, FixedOffset, Offset, TimeZone, Utc};
use database::entities::{accounts, bundle, prelude::*, seasons};
use eyre::{eyre, Result, WrapErr};
use log::{debug, info};
use rand::Rng;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, FromQueryResult,
    ModelTrait, QueryFilter, QueryOrder, QuerySelect,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct YearAndQuarter {
    year: i32,
    quarter: Quarter,
}

impl YearAndQuarter {
    pub fn from_start(start: DateTime<FixedOffset>) -> Self {
        let year = start.year();
        let (year, quarter) = match start.month() {
            12 => (year, Quarter::Winter),
            1 | 2 => (year - 1, Quarter::Winter),
            3 | 4 | 5 => (year, Quarter::Spring),
            6 | 7 | 8 => (year, Quarter::Summer),
            9 | 10 | 11 => (year, Quarter::Autumn),
            _ => unreachable!(),
        };

        Self { year, quarter }
    }

    pub fn start(&self) -> DateTime<FixedOffset> {
        let month = match self.quarter {
            Quarter::Winter => 1,
            Quarter::Spring => 4,
            Quarter::Summer => 7,
            Quarter::Autumn => 10,
        };

        Utc.with_ymd_and_hms(self.year, month, 1, 12, 0, 0)
            .unwrap()
            .with_timezone(&Utc.fix())
    }

    pub fn next(&self) -> Self {
        let (year, quarter) = match self.quarter {
            Quarter::Winter => (self.year + 1, Quarter::Spring),
            Quarter::Spring => (self.year, Quarter::Summer),
            Quarter::Summer => (self.year, Quarter::Autumn),
            Quarter::Autumn => (self.year, Quarter::Winter),
        };

        Self { year, quarter }
    }

    pub fn end(&self) -> DateTime<FixedOffset> {
        self.next().start()
    }
}

impl Display for YearAndQuarter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.quarter, self.year)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Quarter {
    Winter,
    Spring,
    Summer,
    Autumn,
}

impl Display for Quarter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Quarter::Winter => "Winter",
            Quarter::Spring => "Spring",
            Quarter::Summer => "Summer",
            Quarter::Autumn => "Autumn",
        };
        write!(f, "{name}")
    }
}

#[cfg(test)]
mod year_and_quarter_tests {
    use chrono::{DateTime, Offset, Utc};

    use crate::{Quarter, YearAndQuarter};

    #[test]
    fn test_from_start() {
        let date = DateTime::parse_from_rfc3339("2020-01-01T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc.fix());

        let year_and_quarter = YearAndQuarter::from_start(date);

        assert_eq!(year_and_quarter.year, 2019);
        assert_eq!(year_and_quarter.quarter, Quarter::Winter);
    }
}

async fn create_season(
    db: &DatabaseConnection,
    name: String,
    start: DateTime<FixedOffset>,
    end: DateTime<FixedOffset>,
    bundle: bundle::Model,
) -> Result<()> {
    info!("Creating season {} ({:?} - {:?})", name, start, end);

    Seasons::insert(seasons::ActiveModel {
        name: ActiveValue::set(name),
        start: ActiveValue::set(start),
        end: ActiveValue::set(Some(end)),
        bundle_id: ActiveValue::set(bundle.id),
        ..Default::default()
    })
    .exec(db)
    .await?;

    Ok(())
}

pub async fn create_next_season(db: &DatabaseConnection) -> Result<()> {
    let Some(latest_season) = Seasons::find()
        .order_by_asc(seasons::Column::Start)
        .one(db)
        .await? else {
        return Err(eyre!("No season found"))
    };
    let Some(last_used_bundle) = latest_season.find_related(Bundle).one(db).await? else {
        return Err(eyre!("No bundle found for season {}", latest_season.name))
    };

    debug!("Latest season: {:?}", latest_season.name);

    // handle legacy season
    let start = if latest_season.end.is_none() {
        Utc::now().with_timezone(&Utc.fix())
    } else {
        latest_season.start
    };

    let quarter = YearAndQuarter::from_start(start).next();

    create_season(
        db,
        quarter.to_string(),
        quarter.start(),
        quarter.end(),
        last_used_bundle,
    )
    .await?;

    Ok(())
}

pub async fn get_fishes(db: &DatabaseConnection, season: &seasons::Model) -> Result<Vec<Fish>> {
    let Some(bundle) = season.find_related(Bundle).one(db).await? else {
        return Err(eyre!("No bundle found for season {}", season.name))
    };

    let fishes = bundle.find_related(Fishes).all(db).await?;

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
