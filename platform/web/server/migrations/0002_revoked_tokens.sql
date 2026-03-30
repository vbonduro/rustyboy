CREATE TABLE IF NOT EXISTS revoked_tokens (
    jti        TEXT    PRIMARY KEY NOT NULL,
    expires_at INTEGER NOT NULL
);
