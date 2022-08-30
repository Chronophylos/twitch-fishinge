check:
  cargo clippy
  
run-web:
  RUST_LOG=debug cargo run --bin web-fishinge
  
run-bot:
  CHANNELS=chronophylosbot RUST_LOG=debug cargo run --bin twitch-fishinge

docker:
  cargo clippy -- -D warnings
  cargo sqlx prepare --merged
  docker build -t twitch-fishinge:latest -f twitch-fishinge.Dockerfile docker
  docker build -t web-fishinge:latest -f web-fishinge.Dockerfile docker
  docker save -o twitch-micro-bots.tar twitch-fishinge web-fishinge
