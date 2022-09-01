mod db;

use std::{collections::HashMap, env};

use database::{
    entities::{catches, fishes, prelude::*, users},
    migrate,
};
use db::Db;
use dotenvy::dotenv;
use log::{debug, error};
use rocket::{
    catch, catchers,
    fairing::{self, AdHoc},
    fs::FileServer,
    get,
    http::Status,
    routes, Build, FromForm, Rocket,
};
use rocket_db_pools::{Connection, Database};
use rocket_dyn_templates::{
    context,
    tera::{Result as TeraResult, Value},
    Template,
};
use sea_orm::{
    ColumnTrait, DeriveColumn, EntityTrait, EnumIter, FromQueryResult, IdenStatic, JoinType,
    QueryFilter, QueryOrder, QuerySelect, RelationTrait,
};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("Environent variable {name} not set")]
    EnvarNotSet {
        source: std::env::VarError,
        name: &'static str,
    },

    #[error("Joining task failed")]
    JoinTask(#[from] tokio::task::JoinError),
}

#[inline]
fn env_var(name: &'static str) -> Result<String, Error> {
    env::var(name).map_err(|source| Error::EnvarNotSet { source, name })
}

#[rocket::main]
async fn main() -> Result<(), eyre::Error> {
    pretty_env_logger::init_timed();
    dotenv().ok();

    let _rocket = rocket()?.launch().await?;

    Ok(())
}

async fn run_migrations(rocket: Rocket<Build>) -> fairing::Result {
    let conn = &Db::fetch(&rocket).unwrap().conn;
    if let Err(err) = migrate(conn).await {
        error!("Error migrating database {err}");
    }
    Ok(rocket)
}

fn round<const N: usize>(value: &Value, _args: &HashMap<String, Value>) -> TeraResult<Value> {
    match value {
        Value::Number(n) => {
            let x = n.as_f64().unwrap();
            Ok(Value::String(format!("{x:.N$}")))
        }
        _ => Ok(value.clone()),
    }
}

fn rocket() -> Result<Rocket<Build>, Error> {
    let figment = rocket::Config::figment().merge((
        "databases.postgres",
        rocket_db_pools::Config {
            url: env_var("DATABASE_URL").unwrap(),
            min_connections: None,
            max_connections: 1024,
            connect_timeout: 3,
            idle_timeout: None,
        },
    ));

    let rocket = rocket::custom(figment)
        .attach(Db::init())
        .attach(AdHoc::try_on_ignite("Migrations", run_migrations))
        .attach(Template::custom(|engine| {
            engine.tera.register_filter("round1", round::<1>);
            engine.tera.register_filter("round2", round::<2>);
        }))
        .register("/", catchers![internal_server_error])
        .mount("/", routes![index, leaderboard, get_fishes, user])
        .mount("/", FileServer::from("/app/static"));

    Ok(rocket)
}

#[catch(500)]
fn internal_server_error() -> Template {
    Template::render("code/500", context! {})
}

#[get("/")]
fn index() -> Template {
    Template::render("index", context! {})
}

#[derive(Debug, PartialEq, Default, FromForm)]
struct LeaderboardFilter {
    include_bots: bool,
}

#[get("/leaderboard?<filter>")]
async fn leaderboard(conn: Connection<Db>, filter: LeaderboardFilter) -> Result<Template, Status> {
    #[derive(FromQueryResult, Serialize)]
    struct UserWithScore {
        name: String,
        is_bot: bool,
        score: f32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveColumn)]
    enum QueryAs {
        Score,
    }

    let mut query = Catches::find()
        .join(JoinType::InnerJoin, catches::Relation::Users.def())
        .group_by(users::Column::Id)
        .order_by_desc(catches::Column::Value.sum())
        .select_only()
        .column_as(catches::Column::Value.sum(), QueryAs::Score)
        .column(users::Column::Id)
        .column(users::Column::Name)
        .column(users::Column::IsBot);
    sea_orm::QuerySelect::query(&mut query).conditions(
        !filter.include_bots,
        |q| {
            q.and_where(users::Column::IsBot.eq(false));
        },
        |_| (),
    );

    debug!("Querying leaderboard");
    let users = match query.into_model::<UserWithScore>().all(&*conn).await {
        Ok(users) => users
            .into_iter()
            .filter(|u| u.score.abs() > f32::EPSILON)
            .collect::<Vec<_>>(),
        Err(err) => {
            error!("Error querying leaderboard: {err}");
            return Err(Status::InternalServerError);
        }
    };

    Ok(Template::render("leaderboard", context! {users: &users}))
}

#[get("/fishes")]
async fn get_fishes(conn: Connection<Db>) -> Result<Template, Status> {
    #[derive(Serialize)]
    struct Row {
        html_name: String,
        chance: f32,
        base_value: f32,
        min_weight: f32,
        max_weight: f32,
        is_trash: bool,
    }

    debug!("Querying fishes");
    let fishes = match Fishes::find().all(&*conn).await {
        Ok(fishes) => fishes,
        Err(err) => {
            error!("Error querying fishes: {err}");
            return Err(Status::InternalServerError);
        }
    };

    let population: i32 = fishes.iter().map(|fish| fish.count).sum();

    let mut rows: Vec<_> = fishes
        .into_iter()
        .map(|fish| Row {
            html_name: fish.html_name,
            chance: fish.count as f32 / population as f32,
            base_value: fish.base_value,
            min_weight: fish.min_weight,
            max_weight: fish.max_weight,
            is_trash: fish.is_trash,
        })
        .collect();

    rows.sort_by_key(|row| (row.chance * 10000.0) as u64);
    rows.reverse();

    Ok(Template::render("fishes", context! {fishes: &rows}))
}

#[get("/user/<username>")]
async fn user(conn: Connection<Db>, username: String) -> Result<Template, Status> {
    debug!("Quering user {username}");
    let user = match Users::find()
        .filter(users::Column::Name.eq(username.to_lowercase()))
        .one(&*conn)
        .await
    {
        Ok(Some(user)) => user,
        Ok(None) => return Err(Status::NotFound),
        Err(err) => {
            error!("Error querying user {username}: {err}");
            return Err(Status::InternalServerError);
        }
    };

    #[derive(FromQueryResult, Serialize)]
    struct TopCatch {
        name: String,
        weight: Option<f32>,
        value: f32,
    }

    debug!("Querying top catch");
    let top_catch = match Catches::find()
        .filter(catches::Column::UserId.eq(user.id))
        .order_by_desc(catches::Column::Value)
        .join(JoinType::InnerJoin, catches::Relation::Fishes.def())
        .select_only()
        .column(fishes::Column::Name)
        .column(catches::Column::Value)
        .column(catches::Column::Weight)
        .into_model::<TopCatch>()
        .one(&*conn)
        .await
    {
        Ok(Some(top_catch)) => top_catch,
        Ok(None) => return Err(Status::NotFound),
        Err(err) => {
            error!("Error querying top catch for {username}: {err}");
            return Err(Status::InternalServerError);
        }
    };

    #[derive(Copy, Clone, Debug, EnumIter, DeriveColumn)]
    enum QueryAs {
        Score,
    }

    let total_score: Option<f32> = match Catches::find()
        .filter(catches::Column::UserId.eq(user.id))
        .select_only()
        .column_as(catches::Column::Value.sum(), "score")
        .into_values::<_, QueryAs>()
        .one(&*conn)
        .await
    {
        Ok(Some(score)) => score,
        Ok(None) => return Err(Status::NotFound),
        Err(err) => {
            error!("Error querying score for {username}: {err}");
            return Err(Status::InternalServerError);
        }
    };

    Ok(Template::render(
        "user",
        context! {
            user_name: &user.name,
            total_score: &total_score,
            top_catch: &top_catch,
        },
    ))
}
