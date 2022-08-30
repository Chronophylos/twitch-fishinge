use database::{
    connection,
    entities::{catches, prelude::*, users},
    migrate,
};
use dotenvy::dotenv;
use futures_lite::future::block_on;
use log::{debug, error};
use once_cell::sync::Lazy;
use sea_orm::{
    sea_query::Expr, ColumnTrait, DatabaseConnection, DeriveColumn, EntityTrait, EnumIter,
    FromQueryResult, IdenStatic, JoinType, QueryFilter, QueryOrder, QuerySelect, RelationTrait,
};
use serde::{Deserialize, Serialize};
use tera::{Context, Tera};
use warp::{
    hyper::{Body, Response, StatusCode},
    reply::Html,
    Filter, Reply,
};

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("Could not open database connection")]
    OpenDatabase(#[from] database::Error),

    #[error("Database error")]
    Database(#[from] sea_orm::DbErr),

    #[error("Could not render template")]
    RenderTemplate(#[source] tera::Error),

    #[error("Could not build response")]
    BuildResponse(#[from] warp::http::Error),
}

static TEMPLATES: Lazy<Tera> = Lazy::new(|| {
    //let mut tera = Tera::new("templates/**/*.html").unwrap();
    let mut tera = Tera::default();
    tera.add_raw_templates([
        ("base.html", include_str!("templates/base.html")),
        ("fishes.html", include_str!("templates/fishes.html")),
        ("index.html", include_str!("templates/index.html")),
        ("user.html", include_str!("templates/user.html")),
        (
            "leaderboard.html",
            include_str!("templates/leaderboard.html"),
        ),
    ])
    .unwrap();
    tera
});

#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(default)]
struct LeaderboardQuery {
    include_bots: bool,
}

async fn leaderboard(
    db: &DatabaseConnection,
    query: LeaderboardQuery,
) -> Result<Html<String>, Error> {
    debug!("GET /leaderboard {:?}", query);

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

    debug!("Querying leaderboard");
    let users = Catches::find()
        .column_as(catches::Column::Value.sum(), QueryAs::Score)
        .join(JoinType::InnerJoin, catches::Relation::Users.def())
        .group_by(catches::Column::Id)
        .filter(Expr::col(users::Column::IsBot).eq(query.include_bots))
        .into_model::<UserWithScore>()
        .all(db)
        .await?
        .into_iter()
        .filter(|u| u.score > f32::EPSILON)
        .collect::<Vec<_>>();

    let mut context = Context::new();
    context.insert("users", &users);

    Ok(warp::reply::html(
        TEMPLATES
            .render("leaderboard.html", &context)
            .map_err(Error::RenderTemplate)?,
    ))
}

async fn fishes(db: &DatabaseConnection) -> Result<Html<String>, Error> {
    debug!("GET /fishes");

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
    let fishes = Fishes::find().all(db).await?;

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

    let mut context = Context::new();
    context.insert("fishes", &rows);

    Ok(warp::reply::html(
        TEMPLATES
            .render("fishes.html", &context)
            .map_err(Error::RenderTemplate)?,
    ))
}

async fn user(db: &DatabaseConnection, username: String) -> Result<Response<Body>, Error> {
    debug!("GET /user/{username}");

    debug!("Quering user {username}");
    let user = if let Some(user) = Users::find()
        .filter(users::Column::Name.eq(username.to_lowercase()))
        .one(db)
        .await?
    {
        user
    } else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };

    #[derive(FromQueryResult, Serialize)]
    struct TopCatch {
        name: String,
        weight: Option<f64>,
        value: f64,
    }

    debug!("Querying top catch");
    let top_catch = Catches::find()
        .filter(catches::Column::UserId.eq(user.id))
        .order_by_desc(catches::Column::Value)
        .join(JoinType::InnerJoin, catches::Relation::Fishes.def())
        .group_by(catches::Column::Id)
        .into_model::<TopCatch>()
        .one(db)
        .await?;

    #[derive(Copy, Clone, Debug, EnumIter, DeriveColumn)]
    enum QueryAs {
        Score,
    }

    let total_score: Option<i32> = Catches::find()
        .column_as(catches::Column::Value.sum(), "score")
        .select_only()
        .into_values::<_, QueryAs>()
        .one(db)
        .await?;

    let mut context = Context::new();
    context.insert("user_name", &user.name);
    context.insert("total_score", &total_score);
    context.insert("top_catch", &top_catch);

    Ok(Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(
            TEMPLATES
                .render("user.html", &context)
                .map_err(Error::RenderTemplate)?,
        ))?)
}

fn index() -> Result<Html<String>, Error> {
    debug!("GET /");

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
async fn main() -> eyre::Result<()> {
    pretty_env_logger::init_timed();
    dotenv().ok();

    Ok(main_().await?)
}

async fn main_() -> Result<(), Error> {
    let db = connection().await?;
    migrate(&db).await?;

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
        .map({
            let db = (&db).clone();
            move |query: LeaderboardQuery| match block_on(leaderboard(&db, query)) {
                Ok(html) => Box::new(html) as Box<dyn Reply>,
                Err(err) => {
                    error!("Could not render leaderboard: {:?}", err);
                    Box::new(warp::reply::with_status(
                        warp::reply(),
                        StatusCode::INTERNAL_SERVER_ERROR,
                    )) as Box<dyn Reply>
                }
            }
        });

    // GET /fishes
    let fishes_route = warp::path("fishes").map({
        let db = (&db).clone();
        move || match block_on(fishes(&db)) {
            Ok(html) => Box::new(html) as Box<dyn Reply>,
            Err(err) => {
                error!("Could not render fishes: {:?}", err);
                Box::new(warp::reply::with_status(
                    warp::reply(),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )) as Box<dyn Reply>
            }
        }
    });

    // GET /user/:USERNAME
    let user_route = warp::path!("user" / String).map({
        let db = (&db).clone();
        move |username| match block_on(user(&db, username)) {
            Ok(html) => Box::new(html) as Box<dyn Reply>,
            Err(err) => {
                error!("Could not render user: {:?}", err);
                Box::new(warp::reply::with_status(
                    warp::reply(),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )) as Box<dyn Reply>
            }
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
        .and(root.or(leaderboard_route).or(fishes_route).or(user_route))
        .or(assets_route);

    warp::serve(routes).run(([0, 0, 0, 0], 3030)).await;

    Ok(())
}
