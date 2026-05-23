ALTER TABLE save_states ADD COLUMN rom_id TEXT;
ALTER TABLE save_states ADD COLUMN payload_hash TEXT;

CREATE INDEX IF NOT EXISTS idx_save_states_user_rom_id ON save_states(user_id, rom_id);

CREATE TABLE battery_saves_new (
    id           TEXT    PRIMARY KEY NOT NULL,
    user_id      TEXT    NOT NULL REFERENCES users(id),
    rom_id       TEXT,
    rom_name     TEXT    NOT NULL,
    payload_hash TEXT,
    data         BLOB    NOT NULL,
    updated_at   INTEGER NOT NULL
);

INSERT INTO battery_saves_new (id, user_id, rom_name, data, updated_at)
    SELECT id, user_id, rom_name, data, updated_at FROM battery_saves;

DROP TABLE battery_saves;
ALTER TABLE battery_saves_new RENAME TO battery_saves;

CREATE INDEX IF NOT EXISTS idx_battery_saves_user_rom_name
    ON battery_saves(user_id, rom_name);

CREATE UNIQUE INDEX IF NOT EXISTS idx_battery_saves_user_rom_id
    ON battery_saves(user_id, rom_id)
    WHERE rom_id IS NOT NULL;
