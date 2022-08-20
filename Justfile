check:
  cargo clippy

build:
  docker build -t twitch-fishinge:latest .
  docker save -o twitch-fishinge.tar twitch-fishinge
