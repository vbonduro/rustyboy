CREATE TABLE IF NOT EXISTS users (
    id          TEXT    PRIMARY KEY NOT NULL,
    google_sub  TEXT    UNIQUE NOT NULL,
    email       TEXT    NOT NULL,
    display_name TEXT   NOT NULL,
    avatar_url  TEXT,
    created_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS save_states (
    id          TEXT    PRIMARY KEY NOT NULL,
    user_id     TEXT    NOT NULL REFERENCES users(id),
    rom_name    TEXT    NOT NULL,
    slot_name   TEXT    NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    data        BLOB    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_save_states_user_rom ON save_states(user_id, rom_name);

CREATE TABLE IF NOT EXISTS battery_saves (
    id          TEXT    PRIMARY KEY NOT NULL,
    user_id     TEXT    NOT NULL REFERENCES users(id),
    rom_name    TEXT    NOT NULL,
    data        BLOB    NOT NULL,
    updated_at  INTEGER NOT NULL,
    UNIQUE(user_id, rom_name)
);
