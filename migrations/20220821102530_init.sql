-- Add migration script here
CREATE TABLE users(
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    name TEXT NOT NULL,
    last_fished DATETIME NOT NULL,
    score REAL NOT NULL
);

CREATE UNIQUE INDEX "user_names" ON "users" (
	"name"
);