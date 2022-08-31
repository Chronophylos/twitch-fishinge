check:
  cargo clippy
  
run-web:
  RUST_LOG=debug cargo run --bin fishinge-web
  
run-bot:
  CHANNELS=chronophylosbot RUST_LOG=debug cargo run --bin fishinge-bot

run:
  cargo clippy -- -D warnings
  docker compose up --build --abort-on-container-exit

docker:
  cargo clippy -- -D warnings
  docker build -t fishinge-bot:latest -f docker/fishinge-bot.Dockerfile .
  docker build -t fishinge-web:latest -f docker/fishinge-web.Dockerfile .
  docker save -o twitch-micro-bots.tar fishinge-bot fishinge-web
