check:
  cargo clippy
  
run-web:
  RUST_LOG=debug cargo run --bin webserver
  
run-bot:
  RUST_LOG=debug cargo run --bin twitch-fishinge

docker:
  cargo clippy -- -D warnings
  cargo sqlx prepare --merged
  docker build -t twitch-fishinge:latest -f Dockerfile.bot .
  docker build -t twitch-fishinge-web:latest -f Dockerfile.web .
  docker save -o twitch-fishinge.tar twitch-fishinge twitch-fishinge-web
