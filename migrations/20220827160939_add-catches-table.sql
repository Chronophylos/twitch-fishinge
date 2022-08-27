-- Add migration script here
CREATE TABLE catches(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    caught_at TIMESTAMP NOT NULL,
    fish_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    weight REAL,
    value INTEGER NOT NULL,
    FOREIGN KEY (fish_id) REFERENCES fishes(id),
    FOREIGN KEY (user_id) REFERENCES users(id)
);
