check:
  cargo clippy
  
run-web:
  RUST_LOG=debug cargo run --bin web-fishinge
  
run-bot:
  CHANNELS=chronophylosbot RUST_LOG=debug cargo run --bin twitch-fishinge

run:
  cargo clippy -- -D warnings
  docker compose up --build --abort-on-container-exit

docker:
  cargo clippy -- -D warnings
  docker build -t twitch-fishinge:latest -f twitch-fishinge.Dockerfile docker
  docker build -t fishinge-web:latest -f fishinge-web.Dockerfile docker
  docker save -o twitch-micro-bots.tar twitch-fishinge fishinge-web
