-- Add migration script here
CREATE TABLE fishes(
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    name TEXT NOT NULL,
    count INTEGER NOT NULL,
    max_value INTEGER NOT NULL,
    max_weight REAL NOT NULL,
    min_weight REAL NOT NULL,
    is_trash BOOLEAN NOT NULL
);

CREATE UNIQUE INDEX fish_names ON fishes (
	name
);