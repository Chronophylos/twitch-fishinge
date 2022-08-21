-- Add migration script here
ALTER TABLE users ADD is_bot BOOLEAN NOT NULL DEFAULT (false);