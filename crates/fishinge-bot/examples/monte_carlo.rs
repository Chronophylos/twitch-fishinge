use std::collections::HashMap;

use database::connection;
use fishinge_bot::get_fishes;
use rand::{rngs::StdRng, seq::SliceRandom, thread_rng, SeedableRng};

#[tokio::main]
async fn main() {
    let db = connection().await.unwrap();
    let mut rng = StdRng::from_rng(thread_rng()).unwrap();

    let fishes = get_fishes(&db).await.unwrap();

    if fishes.is_empty() {
        panic!("fishes is empty");
    }

    let result = (0..100000)
        .map(|_| fishes.choose(&mut rng).unwrap().catch())
        .fold(HashMap::new(), |mut acc, catch| {
            let entry = acc.entry(catch.fish_name.clone()).or_insert_with(Vec::new);
            entry.push(catch);
            acc
        });

    let count = result.values().map(|catches| catches.len()).sum::<usize>();

    let avg_catch_value = result
        .values()
        .map(|catches| catches.iter().map(|catch| catch.value).sum::<f32>())
        .sum::<f32>()
        / count as f32;

    println!("Caught {count} fishes");
    println!("Average Catch Value: ${avg_catch_value}");
}
