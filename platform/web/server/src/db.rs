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

        // Look up by sub first, then fall back to email — this merges accounts
        // that were created via different auth paths (CF Access vs Google OAuth).
        let existing = match self.get_user_by_google_sub(google_sub).await? {
            Some(u) => Some(u),
            None => self.get_user_by_email(email).await?,
        };
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

    pub async fn get_user_by_email(&self, email: &str) -> Result<Option<User>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, google_sub, email, display_name, avatar_url, created_at
             FROM users WHERE email = ?",
        )
        .bind(email)
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

    /// Returns the most recent save state for a given user+rom, without the blob data.
    pub async fn get_latest_save_state(&self, user_id: &str, rom_name: &str) -> Result<Option<SaveState>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, user_id, rom_name, slot_name, created_at, updated_at, data
             FROM save_states WHERE user_id = ? AND rom_name = ?
             ORDER BY updated_at DESC LIMIT 1",
        )
        .bind(user_id)
        .bind(rom_name)
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

    pub async fn delete_save_state(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM save_states WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Returns one row per rom_name the user has saves for, with the most recent updated_at.
    pub async fn list_roms_with_saves(&self, user_id: &str) -> Result<Vec<(String, i64)>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT rom_name, MAX(updated_at) as last_saved
             FROM save_states WHERE user_id = ?
             GROUP BY rom_name
             ORDER BY last_saved DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| (r.get("rom_name"), r.get("last_saved"))).collect())
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

    // --- Revoked tokens ---

    pub async fn revoke_token(&self, jti: &str, expires_at: i64) -> Result<(), sqlx::Error> {
        // Also purge expired tokens lazily to keep the table small
        sqlx::query("DELETE FROM revoked_tokens WHERE expires_at < ?")
            .bind(now_secs())
            .execute(&self.pool)
            .await?;
        sqlx::query("INSERT OR IGNORE INTO revoked_tokens (jti, expires_at) VALUES (?, ?)")
            .bind(jti)
            .bind(expires_at)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn is_token_revoked(&self, jti: &str) -> Result<bool, sqlx::Error> {
        let row = sqlx::query("SELECT 1 FROM revoked_tokens WHERE jti = ?")
            .bind(jti)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn new_db() -> Database {
        Database::connect(":memory:").await.expect("in-memory db failed")
    }

    #[tokio::test]
    async fn test_upsert_and_get_user_by_google_sub() {
        let db = new_db().await;
        let user = db
            .upsert_user("sub123", "alice@example.com", "Alice", Some("https://avatar.example.com/alice"))
            .await
            .unwrap();
        assert_eq!(user.google_sub, "sub123");
        assert_eq!(user.email, "alice@example.com");
        assert_eq!(user.display_name, "Alice");
        assert_eq!(user.avatar_url.as_deref(), Some("https://avatar.example.com/alice"));

        let fetched = db.get_user_by_google_sub("sub123").await.unwrap().unwrap();
        assert_eq!(fetched.id, user.id);
        assert_eq!(fetched.google_sub, user.google_sub);
        assert_eq!(fetched.email, user.email);
        assert_eq!(fetched.display_name, user.display_name);
        assert_eq!(fetched.avatar_url, user.avatar_url);
        assert_eq!(fetched.created_at, user.created_at);
    }

    #[tokio::test]
    async fn test_upsert_user_updates_existing() {
        let db = new_db().await;
        db.upsert_user("sub_update", "old@example.com", "OldName", None)
            .await
            .unwrap();
        let updated = db
            .upsert_user("sub_update", "new@example.com", "NewName", Some("https://avatar.example.com/new"))
            .await
            .unwrap();
        assert_eq!(updated.email, "new@example.com");
        assert_eq!(updated.display_name, "NewName");
        assert_eq!(updated.avatar_url.as_deref(), Some("https://avatar.example.com/new"));

        // Only one row should exist
        let row_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE google_sub = 'sub_update'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(row_count, 1);
    }

    #[tokio::test]
    async fn test_get_user_by_id() {
        let db = new_db().await;
        let user = db
            .upsert_user("sub_by_id", "byid@example.com", "ById", None)
            .await
            .unwrap();
        let fetched = db.get_user_by_id(&user.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, user.id);
        assert_eq!(fetched.email, user.email);
        assert_eq!(fetched.display_name, user.display_name);
    }

    #[tokio::test]
    async fn test_get_user_missing() {
        let db = new_db().await;
        let result = db
            .get_user_by_id("00000000-0000-0000-0000-000000000000")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_upsert_save_state_creates_new() {
        let db = new_db().await;
        let user = db
            .upsert_user("sub_ss_new", "ss@example.com", "SSUser", None)
            .await
            .unwrap();
        let data = vec![1u8, 2, 3, 4, 5];
        let ss = db
            .upsert_save_state(&user.id, "tetris.gb", "slot1", data.clone())
            .await
            .unwrap();
        assert_eq!(ss.user_id, user.id);
        assert_eq!(ss.rom_name, "tetris.gb");
        assert_eq!(ss.slot_name, "slot1");
        assert_eq!(ss.data, data);
        assert!(!ss.id.is_empty());
    }

    #[tokio::test]
    async fn test_upsert_save_state_updates_existing() {
        let db = new_db().await;
        let user = db
            .upsert_user("sub_ss_upd", "ssupd@example.com", "SSUpd", None)
            .await
            .unwrap();
        db.upsert_save_state(&user.id, "tetris.gb", "slot1", vec![1, 2, 3])
            .await
            .unwrap();
        let new_data = vec![9u8, 8, 7];
        let updated = db
            .upsert_save_state(&user.id, "tetris.gb", "slot1", new_data.clone())
            .await
            .unwrap();
        assert_eq!(updated.data, new_data);

        // Only one row should exist
        let row_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM save_states WHERE user_id = ? AND rom_name = 'tetris.gb' AND slot_name = 'slot1'",
        )
        .bind(&user.id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(row_count, 1);
    }

    #[tokio::test]
    async fn test_list_save_states() {
        let db = new_db().await;
        let user = db
            .upsert_user("sub_ss_list", "sslist@example.com", "SSList", None)
            .await
            .unwrap();
        db.upsert_save_state(&user.id, "zelda.gb", "slot1", vec![1]).await.unwrap();
        db.upsert_save_state(&user.id, "zelda.gb", "slot2", vec![2]).await.unwrap();
        db.upsert_save_state(&user.id, "zelda.gb", "slot3", vec![3]).await.unwrap();
        let states = db.list_save_states(&user.id, "zelda.gb").await.unwrap();
        assert_eq!(states.len(), 3);
    }

    #[tokio::test]
    async fn test_list_save_states_empty() {
        let db = new_db().await;
        let states = db
            .list_save_states("00000000-0000-0000-0000-000000000000", "none.gb")
            .await
            .unwrap();
        assert!(states.is_empty());
    }

    #[tokio::test]
    async fn test_get_save_state_by_id() {
        let db = new_db().await;
        let user = db
            .upsert_user("sub_ss_get", "ssget@example.com", "SSGet", None)
            .await
            .unwrap();
        let data = vec![42u8, 43, 44];
        let ss = db
            .upsert_save_state(&user.id, "mario.gb", "slot1", data.clone())
            .await
            .unwrap();
        let fetched = db.get_save_state(&ss.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, ss.id);
        assert_eq!(fetched.data, data);
        assert_eq!(fetched.rom_name, "mario.gb");
    }

    #[tokio::test]
    async fn test_upsert_battery_save_creates_and_updates() {
        let db = new_db().await;
        let user = db
            .upsert_user("sub_bat", "bat@example.com", "BatUser", None)
            .await
            .unwrap();
        let first_data = vec![10u8, 20, 30];
        let bs1 = db
            .upsert_battery_save(&user.id, "pokemon.gb", first_data.clone())
            .await
            .unwrap();
        assert_eq!(bs1.data, first_data);

        let second_data = vec![99u8, 88, 77];
        let bs2 = db
            .upsert_battery_save(&user.id, "pokemon.gb", second_data.clone())
            .await
            .unwrap();
        assert_eq!(bs2.id, bs1.id);
        assert_eq!(bs2.data, second_data);

        // Only one row
        let row_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM battery_saves WHERE user_id = ? AND rom_name = 'pokemon.gb'",
        )
        .bind(&user.id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(row_count, 1);
    }

    #[tokio::test]
    async fn test_get_battery_save_missing() {
        let db = new_db().await;
        let result = db
            .get_battery_save("00000000-0000-0000-0000-000000000000", "none.gb")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_user_by_email_found() {
        let db = new_db().await;
        let user = db
            .upsert_user("sub_email", "vincent@example.com", "Vincent", None)
            .await
            .unwrap();
        let fetched = db.get_user_by_email("vincent@example.com").await.unwrap().unwrap();
        assert_eq!(fetched.id, user.id);
        assert_eq!(fetched.email, "vincent@example.com");
        assert_eq!(fetched.display_name, "Vincent");
    }

    #[tokio::test]
    async fn test_get_user_by_email_missing() {
        let db = new_db().await;
        let result = db.get_user_by_email("nobody@example.com").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_google_then_cf_same_account() {
        // Google login first → CF login second: same record, same display name.
        let db = new_db().await;

        let google_user = db
            .upsert_user("google-sub-abc", "vincent@example.com", "vincent", None)
            .await
            .unwrap();

        let cf_user = db
            .upsert_user("cf-sub-xyz", "vincent@example.com", "vincent", None)
            .await
            .unwrap();

        assert_eq!(cf_user.id, google_user.id);
        assert_eq!(cf_user.display_name, "vincent");
    }

    #[tokio::test]
    async fn test_cf_then_google_same_account() {
        // CF login first → Google login second: same record, same display name.
        let db = new_db().await;

        let cf_user = db
            .upsert_user("cf-sub-xyz", "vincent@example.com", "vincent", None)
            .await
            .unwrap();

        let google_user = db
            .upsert_user("google-sub-abc", "vincent@example.com", "vincent", None)
            .await
            .unwrap();

        assert_eq!(google_user.id, cf_user.id);
        assert_eq!(google_user.display_name, "vincent");
    }

    #[tokio::test]
    async fn test_cf_login_creates_user_with_email_local_part() {
        // When no existing user exists, CF login creates one using the email local-part.
        let db = new_db().await;

        let display_name = "vbonduro@example.com".split('@').next().unwrap_or("vbonduro@example.com");
        let user = db
            .upsert_user("cf-sub-new", "vbonduro@example.com", display_name, None)
            .await
            .unwrap();

        assert_eq!(user.display_name, "vbonduro");
        assert_eq!(user.email, "vbonduro@example.com");
    }

    #[tokio::test]
    async fn test_get_latest_save_state_returns_most_recent() {
        let db = new_db().await;
        let user = db
            .upsert_user("sub_latest", "latest@example.com", "Latest", None)
            .await
            .unwrap();
        db.upsert_save_state(&user.id, "link.gb", "slot1", vec![1]).await.unwrap();
        db.upsert_save_state(&user.id, "link.gb", "slot2", vec![2]).await.unwrap();
        // Force slot2 to have a clearly later updated_at so ORDER BY is deterministic
        sqlx::query("UPDATE save_states SET updated_at = updated_at + 10 WHERE slot_name = 'slot2'")
            .execute(&db.pool)
            .await
            .unwrap();

        let latest = db.get_latest_save_state(&user.id, "link.gb").await.unwrap().unwrap();
        assert_eq!(latest.slot_name, "slot2");
        assert_eq!(latest.data, vec![2]);
    }

    #[tokio::test]
    async fn test_get_latest_save_state_missing() {
        let db = new_db().await;
        let result = db
            .get_latest_save_state("00000000-0000-0000-0000-000000000000", "none.gb")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_list_roms_with_saves_returns_distinct_roms() {
        let db = new_db().await;
        let user = db
            .upsert_user("sub_roms", "roms@example.com", "Roms", None)
            .await
            .unwrap();
        db.upsert_save_state(&user.id, "tetris.gb", "slot1", vec![1]).await.unwrap();
        db.upsert_save_state(&user.id, "tetris.gb", "slot2", vec![2]).await.unwrap();
        db.upsert_save_state(&user.id, "mario.gb",  "slot1", vec![3]).await.unwrap();

        let roms = db.list_roms_with_saves(&user.id).await.unwrap();
        assert_eq!(roms.len(), 2);
        let names: Vec<&str> = roms.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"tetris.gb"));
        assert!(names.contains(&"mario.gb"));
    }

    #[tokio::test]
    async fn test_list_roms_with_saves_empty() {
        let db = new_db().await;
        let roms = db
            .list_roms_with_saves("00000000-0000-0000-0000-000000000000")
            .await
            .unwrap();
        assert!(roms.is_empty());
    }

    #[tokio::test]
    async fn test_delete_save_state() {
        let db = new_db().await;
        let user = db
            .upsert_user("sub_del", "del@example.com", "Del", None)
            .await
            .unwrap();
        let ss = db
            .upsert_save_state(&user.id, "wario.gb", "slot1", vec![7, 8, 9])
            .await
            .unwrap();

        // Verify it exists
        assert!(db.get_save_state(&ss.id).await.unwrap().is_some());

        // Delete it
        db.delete_save_state(&ss.id).await.unwrap();

        // Verify it's gone
        assert!(db.get_save_state(&ss.id).await.unwrap().is_none());
    }
}
