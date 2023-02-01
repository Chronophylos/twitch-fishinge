use std::iter;

use database::connection;
use fishinge_bot::get_fishes;
use indicatif::ProgressBar;
use rand::{rngs::StdRng, seq::SliceRandom, thread_rng, SeedableRng};

#[tokio::main]
async fn main() {
    let db = connection().await.unwrap();
    let mut rng = StdRng::from_rng(thread_rng()).unwrap();

    let fishes = get_fishes(&db).await.unwrap();

    if fishes.is_empty() {
        panic!("fishes is empty");
    }

    let bar = ProgressBar::new(100_000);

    let avg = bar
        .wrap_iter(0..100_000)
        .map(|_| {
            let (count, _) = iter::repeat(())
                .map(|_| fishes.choose(&mut rng).unwrap().catch())
                .enumerate()
                .find(|(_, catch)| catch.fish_name == "FLOPPA")
                .unwrap();
            count
        })
        .sum::<usize>()
        / 100_000;

    bar.finish();

    println!("Took {avg} tries on average to catch a FLOPPA");
}
