use sqlx::{SqlitePool, sqlite::SqliteConnectOptions, Row};
use std::{str::FromStr, time::{SystemTime, UNIX_EPOCH}};

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub google_sub: String,
    pub email: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct SaveState {
    pub id: String,
    pub user_id: String,
    pub rom_name: String,
    pub slot_name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct BatterySave {
    pub id: String,
    pub user_id: String,
    pub rom_name: String,
    pub data: Vec<u8>,
    pub updated_at: i64,
}

#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    pub async fn connect(path: &str) -> Result<Self, sqlx::Error> {
        let opts = SqliteConnectOptions::from_str(&format!("sqlite:{path}"))?
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await?;

        // Run embedded migrations
        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self { pool })
    }

    // --- Users ---

    pub async fn upsert_user(
        &self,
        google_sub: &str,
        email: &str,
        display_name: &str,
        avatar_url: Option<&str>,
    ) -> Result<User, sqlx::Error> {
        let now = now_secs();

        // Try to update existing user first
        let existing = self.get_user_by_google_sub(google_sub).await?;
        if let Some(mut user) = existing {
            sqlx::query(
                "UPDATE users SET email = ?, display_name = ?, avatar_url = ? WHERE id = ?",
            )
            .bind(email)
            .bind(display_name)
            .bind(avatar_url)
            .bind(&user.id)
            .execute(&self.pool)
            .await?;
            user.email = email.to_string();
            user.display_name = display_name.to_string();
            user.avatar_url = avatar_url.map(str::to_string);
            return Ok(user);
        }

        let id = new_id();
        sqlx::query(
            "INSERT INTO users (id, google_sub, email, display_name, avatar_url, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(google_sub)
        .bind(email)
        .bind(display_name)
        .bind(avatar_url)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(User {
            id,
            google_sub: google_sub.to_string(),
            email: email.to_string(),
            display_name: display_name.to_string(),
            avatar_url: avatar_url.map(str::to_string),
            created_at: now,
        })
    }

    pub async fn get_user_by_google_sub(&self, google_sub: &str) -> Result<Option<User>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, google_sub, email, display_name, avatar_url, created_at
             FROM users WHERE google_sub = ?",
        )
        .bind(google_sub)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| User {
            id: r.get("id"),
            google_sub: r.get("google_sub"),
            email: r.get("email"),
            display_name: r.get("display_name"),
            avatar_url: r.get("avatar_url"),
            created_at: r.get("created_at"),
        }))
    }

    pub async fn get_user_by_id(&self, id: &str) -> Result<Option<User>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, google_sub, email, display_name, avatar_url, created_at
             FROM users WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| User {
            id: r.get("id"),
            google_sub: r.get("google_sub"),
            email: r.get("email"),
            display_name: r.get("display_name"),
            avatar_url: r.get("avatar_url"),
            created_at: r.get("created_at"),
        }))
    }

    // --- Save States ---

    pub async fn upsert_save_state(
        &self,
        user_id: &str,
        rom_name: &str,
        slot_name: &str,
        data: Vec<u8>,
    ) -> Result<SaveState, sqlx::Error> {
        let now = now_secs();

        // Check if a save state already exists for this slot
        let existing = sqlx::query(
            "SELECT id, created_at FROM save_states
             WHERE user_id = ? AND rom_name = ? AND slot_name = ?",
        )
        .bind(user_id)
        .bind(rom_name)
        .bind(slot_name)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = existing {
            let id: String = row.get("id");
            let created_at: i64 = row.get("created_at");
            sqlx::query(
                "UPDATE save_states SET data = ?, updated_at = ? WHERE id = ?",
            )
            .bind(&data)
            .bind(now)
            .bind(&id)
            .execute(&self.pool)
            .await?;
            return Ok(SaveState {
                id,
                user_id: user_id.to_string(),
                rom_name: rom_name.to_string(),
                slot_name: slot_name.to_string(),
                created_at,
                updated_at: now,
                data,
            });
        }

        let id = new_id();
        sqlx::query(
            "INSERT INTO save_states (id, user_id, rom_name, slot_name, created_at, updated_at, data)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(user_id)
        .bind(rom_name)
        .bind(slot_name)
        .bind(now)
        .bind(now)
        .bind(&data)
        .execute(&self.pool)
        .await?;

        Ok(SaveState {
            id,
            user_id: user_id.to_string(),
            rom_name: rom_name.to_string(),
            slot_name: slot_name.to_string(),
            created_at: now,
            updated_at: now,
            data,
        })
    }

    pub async fn list_save_states(
        &self,
        user_id: &str,
        rom_name: &str,
    ) -> Result<Vec<SaveState>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, user_id, rom_name, slot_name, created_at, updated_at, data
             FROM save_states WHERE user_id = ? AND rom_name = ?
             ORDER BY updated_at DESC",
        )
        .bind(user_id)
        .bind(rom_name)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| SaveState {
                id: r.get("id"),
                user_id: r.get("user_id"),
                rom_name: r.get("rom_name"),
                slot_name: r.get("slot_name"),
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
                data: r.get("data"),
            })
            .collect())
    }

    pub async fn get_save_state(&self, id: &str) -> Result<Option<SaveState>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, user_id, rom_name, slot_name, created_at, updated_at, data
             FROM save_states WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| SaveState {
            id: r.get("id"),
            user_id: r.get("user_id"),
            rom_name: r.get("rom_name"),
            slot_name: r.get("slot_name"),
            created_at: r.get("created_at"),
            updated_at: r.get("updated_at"),
            data: r.get("data"),
        }))
    }

    // --- Battery Saves ---

    pub async fn upsert_battery_save(
        &self,
        user_id: &str,
        rom_name: &str,
        data: Vec<u8>,
    ) -> Result<BatterySave, sqlx::Error> {
        let now = now_secs();

        let existing = sqlx::query(
            "SELECT id FROM battery_saves WHERE user_id = ? AND rom_name = ?",
        )
        .bind(user_id)
        .bind(rom_name)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = existing {
            let id: String = row.get("id");
            sqlx::query(
                "UPDATE battery_saves SET data = ?, updated_at = ? WHERE id = ?",
            )
            .bind(&data)
            .bind(now)
            .bind(&id)
            .execute(&self.pool)
            .await?;
            return Ok(BatterySave {
                id,
                user_id: user_id.to_string(),
                rom_name: rom_name.to_string(),
                data,
                updated_at: now,
            });
        }

        let id = new_id();
        sqlx::query(
            "INSERT INTO battery_saves (id, user_id, rom_name, data, updated_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(user_id)
        .bind(rom_name)
        .bind(&data)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(BatterySave {
            id,
            user_id: user_id.to_string(),
            rom_name: rom_name.to_string(),
            data,
            updated_at: now,
        })
    }

    pub async fn get_battery_save(
        &self,
        user_id: &str,
        rom_name: &str,
    ) -> Result<Option<BatterySave>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, user_id, rom_name, data, updated_at
             FROM battery_saves WHERE user_id = ? AND rom_name = ?",
        )
        .bind(user_id)
        .bind(rom_name)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| BatterySave {
            id: r.get("id"),
            user_id: r.get("user_id"),
            rom_name: r.get("rom_name"),
            data: r.get("data"),
            updated_at: r.get("updated_at"),
        }))
    }
}
