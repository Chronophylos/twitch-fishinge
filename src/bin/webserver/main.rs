use std::collections::HashMap;

use futures_lite::future::block_on;
use log::error;
use once_cell::sync::Lazy;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tera::{Context, Tera, Value};
use twitch_fishinge::{
    db_conn,
    models::{Fish, User},
};
use warp::{reply::Html, Filter, Reply};

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("Could not open database connection")]
    OpenDatabase(#[from] twitch_fishinge::OpenDatabaseError),

    #[error("Could not query users")]
    QueryUsers(#[source] sqlx::Error),

    #[error("Could not query fishes")]
    QueryFishes(#[source] sqlx::Error),

    #[error("Could not render template")]
    RenderTemplate(#[source] tera::Error),
}

fn score_formatter(value: &Value, _args: &HashMap<String, Value>) -> tera::Result<Value> {
    match value {
        Value::Number(n) if n.is_f64() => Ok(Value::String(format!("${:.2}", n.as_f64().unwrap()))),
        Value::Number(n) if n.is_i64() => Ok(Value::String(format!("${}.00", n.as_i64().unwrap()))),
        _ => Err(tera::Error::msg("Could not format score")),
    }
}

fn percentage_formatter(value: &Value, _args: &HashMap<String, Value>) -> tera::Result<Value> {
    match value {
        Value::Number(n) if n.is_f64() => Ok(Value::String(format!("{:.2}%", n.as_f64().unwrap()))),
        _ => Err(tera::Error::msg("Could not format percentage")),
    }
}

static TEMPLATES: Lazy<Tera> = Lazy::new(|| {
    let mut tera = Tera::new("templates/**/*.html").unwrap();
    tera.register_filter("score", score_formatter);
    tera.register_filter("percentage", percentage_formatter);
    tera
});

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct LeaderboardQuery {
    include_bots: bool,
}

async fn leaderboard(query: LeaderboardQuery) -> Result<Html<String>, Error> {
    let mut conn = db_conn().await?;

    let users: Vec<_> = sqlx::query_as!(User, "SELECT * FROM users ORDER BY score DESC")
        .fetch_all(&mut conn)
        .await
        .map_err(Error::QueryUsers)?
        .into_iter()
        .filter(|user| user.score > 0.0)
        .filter(|user| !user.is_bot || query.include_bots)
        .collect();

    let mut context = Context::new();
    context.insert("users", &users);

    Ok(warp::reply::html(
        TEMPLATES
            .render("leaderboard.html", &context)
            .map_err(Error::RenderTemplate)?,
    ))
}

async fn fishes() -> Result<Html<String>, Error> {
    #[derive(Serialize)]
    struct Row {
        name: String,
        count: i64,
        chance: f64,
        base_value: i64,
        min_weight: f64,
        max_weight: f64,
        is_trash: bool,
    }

    let mut conn = db_conn().await?;

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
            base_value: fish.base_value,
            min_weight: fish.min_weight,
            max_weight: fish.max_weight,
            is_trash: fish.is_trash,
        })
        .collect();

    rows.sort_by_key(|row| (row.chance * 10000.0) as u64);
    rows.reverse();

    let mut context = Context::new();
    context.insert("fishes", &rows);

    Ok(warp::reply::html(
        TEMPLATES
            .render("fishes.html", &context)
            .map_err(Error::RenderTemplate)?,
    ))
}

fn index() -> Result<Html<String>, Error> {
    Ok(warp::reply::html(
        TEMPLATES
            .render("index.html", &Context::new())
            .map_err(Error::RenderTemplate)?,
    ))
}

macro_rules! assets {
    {$first_file:literal => $first_content_type:literal, $($file:literal => $content_type:literal),*} => {
        warp::get().or(warp::head()).unify().and({
            let f = warp::path($first_file)
                .map(|| { assets!(BUILDER, $first_file, $first_content_type) });
                $(
                    let f = f.or(warp::path($file).map(|| { assets!(BUILDER, $file, $content_type) }));
                )*
            f
        })
    };
    (BUILDER, $file:literal, $content_type:literal) => {
        ::warp::hyper::Response::builder()
            .header("ContentType", $content_type)
            .header("Cache-Control", "public, max-age=31536000")
            .body(::warp::hyper::Body::from(include_bytes!(concat!("assets/", $file)).as_slice()))
    };
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    pretty_env_logger::init();

    // GET /
    let root = warp::path::end().map(|| match index() {
        Ok(html) => Box::new(html) as Box<dyn Reply>,
        Err(err) => {
            error!("Could not render index: {:?}", err);
            Box::new(warp::reply::with_status(
                warp::reply(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )) as Box<dyn Reply>
        }
    });

    // GET /leaderboard
    let leaderboard_route = warp::path("leaderboard")
        .and(warp::query::<LeaderboardQuery>())
        .map(
            |query: LeaderboardQuery| match block_on(leaderboard(query)) {
                Ok(html) => Box::new(html) as Box<dyn Reply>,
                Err(err) => {
                    error!("Could not render leaderboard: {:?}", err);
                    Box::new(warp::reply::with_status(
                        warp::reply(),
                        StatusCode::INTERNAL_SERVER_ERROR,
                    )) as Box<dyn Reply>
                }
            },
        );

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

    let assets_route = assets! {
        "android-chrome-144x144.png" => "image/png",
        "apple-touch-icon.png" => "image/png",
        "browserconfig.xml" => "application/xml",
        "favicon-16x16.png" => "image/png",
        "favicon-32x32.png" => "image/png",
        "favicon.ico" => "image/x-icon",
        "mstile-150x150.png" => "image/png",
        "nerdge-large.webp" => "image/webp",
        "nerdge-small.webp" => "image/webp",
        "safari-pinned-tab.svg" => "image/svg+xml",
        "site.webmanifest" => "application/manifest+json",
        "Neonderthaw-Regular.ttf" => "application/x-font-ttf",
        "Righteous-Regular.ttf" => "application/x-font-ttf",
        "style.css" => "text/css"
    };

    let routes = warp::get()
        .and(root.or(leaderboard_route).or(fishes_route))
        .or(assets_route);

    warp::serve(routes).run(([0, 0, 0, 0], 3030)).await;

    Ok(())
}
