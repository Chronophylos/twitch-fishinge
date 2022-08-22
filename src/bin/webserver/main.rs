use std::fmt::Write;

use futures_lite::future::block_on;
use log::error;
use reqwest::StatusCode;
use serde::Serialize;
use serde_json::Value;
use tinytemplate::{format, TinyTemplate};
use twitch_fishinge::{
    db_conn,
    models::{Fish, User},
};
use warp::{reply::Html, Filter, Reply};

const LEADERBOARD: &str = include_str!("html/leaderboard.html");
const FISHES: &str = include_str!("html/fishes.html");

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("Could not open database connection")]
    OpenDatabase(#[from] twitch_fishinge::OpenDatabaseError),

    #[error("Could not query users")]
    QueryUsers(#[source] sqlx::Error),

    #[error("Could not query fishes")]
    QueryFishes(#[source] sqlx::Error),

    #[error("Could not compile template")]
    CompileTemplate(#[source] tinytemplate::error::Error),

    #[error("Could not render template")]
    RenderTemplate(#[source] tinytemplate::error::Error),
}

fn score_formatter(value: &Value, output: &mut String) -> Result<(), tinytemplate::error::Error> {
    match value {
        Value::Number(n) if n.is_f64() => {
            write!(output, "${:.2}", n.as_f64().unwrap())?;
            Ok(())
        }
        Value::Number(n) if n.is_i64() => {
            write!(output, "${}.00", n.as_i64().unwrap())?;
            Ok(())
        }
        _ => format(value, output),
    }
}

async fn leaderboard() -> Result<Html<String>, Error> {
    #[derive(Serialize)]
    struct Context {
        users: Vec<(usize, User)>,
    }

    let mut conn = db_conn().await?;

    let mut tt = TinyTemplate::new();
    tt.add_template("leaderboard", LEADERBOARD)
        .map_err(Error::CompileTemplate)?;
    tt.add_formatter("score_formatter", score_formatter);

    let users: Vec<_> = sqlx::query_as!(User, "SELECT * FROM users ORDER BY score DESC")
        .fetch_all(&mut conn)
        .await
        .map_err(Error::QueryUsers)?
        .into_iter()
        .filter(|user| user.score > 0.0)
        .filter(|user| !user.is_bot)
        .enumerate()
        .map(|(i, user)| (i + 1, user))
        .collect();

    let context = Context { users };

    Ok(warp::reply::html(
        tt.render("leaderboard", &context)
            .map_err(Error::RenderTemplate)?,
    ))
}

async fn fishes() -> Result<Html<String>, Error> {
    #[derive(Serialize)]
    struct Context {
        fishes: Vec<Row>,
    }

    #[derive(Serialize)]
    struct Row {
        name: String,
        count: i64,
        chance: f64,
        max_value: i64,
        min_weight: f64,
        max_weight: f64,
        is_trash: bool,
    }

    let mut conn = db_conn().await?;

    let mut tt = TinyTemplate::new();
    tt.add_template("fishes", FISHES)
        .map_err(Error::CompileTemplate)?;
    tt.add_formatter("score_formatter", score_formatter);
    tt.add_formatter("percentage_formatter", |value, output| match value {
        Value::Number(n) if n.is_f64() => {
            write!(output, "{:.2}%", n.as_f64().unwrap() * 100.0)?;
            Ok(())
        }
        _ => format(value, output),
    });

    let fishes = sqlx::query_as!(Fish, "SELECT * FROM fishes")
        .fetch_all(&mut conn)
        .await
        .map_err(Error::QueryFishes)?;

    let population: i64 = fishes.iter().map(|fish| fish.count).sum();

    let mut rows: Vec<_> = fishes
        .into_iter()
        .map(|fish| Row {
            name: fish.name,
            count: fish.count,
            chance: fish.count as f64 / population as f64,
            max_value: fish.max_value,
            min_weight: fish.min_weight,
            max_weight: fish.max_weight,
            is_trash: fish.is_trash,
        })
        .collect();

    rows.sort_by_key(|row| (row.chance * 10000.0) as u64);
    rows.reverse();

    let context = Context { fishes: rows };

    Ok(warp::reply::html(
        tt.render("fishes", &context)
            .map_err(Error::RenderTemplate)?,
    ))
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    pretty_env_logger::init();

    // GET /
    let root = warp::path::end().map(|| warp::reply::html(include_str!("html/index.html")));

    // GET /leaderboard
    let leaderboard_route = warp::path("leaderboard").map(|| match block_on(leaderboard()) {
        Ok(html) => Box::new(html) as Box<dyn Reply>,
        Err(err) => {
            error!("Could not render leaderboard: {:?}", err);
            Box::new(warp::reply::with_status(
                warp::reply(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )) as Box<dyn Reply>
        }
    });

    // GET /fishes
    let fishes_route = warp::path("fishes").map(|| match block_on(fishes()) {
        Ok(html) => Box::new(html) as Box<dyn Reply>,
        Err(err) => {
            error!("Could not render fishes: {:?}", err);
            Box::new(warp::reply::with_status(
                warp::reply(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )) as Box<dyn Reply>
        }
    });

    let routes = warp::get().and(root.or(leaderboard_route).or(fishes_route));

    warp::serve(routes).run(([0, 0, 0, 0], 3030)).await;

    Ok(())
}
