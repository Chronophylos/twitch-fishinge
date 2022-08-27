#![allow(dead_code)]

use chrono::NaiveDateTime;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Fish {
    pub id: i64,
    pub name: String,
    pub count: i64,
    pub base_value: i64,
    pub min_weight: f64,
    pub max_weight: f64,
    pub is_trash: bool,
}

#[derive(Debug, Serialize)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub last_fished: NaiveDateTime,
    pub score: f64,
    pub is_bot: bool,
}

#[derive(Debug, Serialize)]
pub struct Catch {
    pub id: i64,
    pub caught_at: NaiveDateTime,
    pub fish: Fish,
    pub user: User,
    pub weight: Option<f64>,
    pub value: f64,
}
