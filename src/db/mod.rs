use anyhow::Result;
use sqlx::{sqlite::SqlitePoolOptions, Pool, Sqlite};
use std::env;
use tracing::info;

use crate::models::{
    Dar20Balance, Dar20Mint, Dar20Token, Dar20Transfer, DashDomain, DashMapClaim, IndexerStats, Inscription,
};

/// Database wrapper
#[derive(Clone)]
pub struct Database {
    pool: Pool<Sqlite>,
}

impl Database {
    /// Create new database connection
    pub async fn new() -> Result<Self> {
        let database_url =
            env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://dash_indexer.db".to_string());

        info!("Connecting to database: {}", database_url);

        let pool = SqlitePoolOptions::new()
            .max_connections(10)
            .connect(&database_url)
            .await?;

        Ok(Self { pool })
    }

    /// Health check - verify database connectivity
    pub async fn health_check(&self) -> Result<()> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Get cached chain height (from indexer state or estimate)
    pub async fn get_chain_height(&self) -> Result<u64> {
        // Try to get from indexer state table if we stored it
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT value FROM indexer_state WHERE key = 'chain_height'"
        )
        .fetch_optional(&self.pool)
        .await?;

        if let Some((height,)) = row {
            return Ok(height as u64);
        }

        // Fallback: estimate from last indexed block
        let last_block = self.get_last_indexed_block().await?;
        Ok(last_block)
    }

    /// Update chain height in state
    pub async fn set_chain_height(&self, height: u64) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO indexer_state (key, value, updated_at) VALUES ('chain_height', ?, CURRENT_TIMESTAMP)"
        )
        .bind(height as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Run database migrations
    pub async fn run_migrations(&self) -> Result<()> {
        info!("Running database migrations...");

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS indexer_state (
                id INTEGER PRIMARY KEY,
                last_indexed_block INTEGER NOT NULL DEFAULT 0,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            INSERT OR IGNORE INTO indexer_state (id, last_indexed_block) VALUES (1, 0);
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS inscriptions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                inscription_id TEXT UNIQUE NOT NULL,
                txid TEXT NOT NULL,
                vout INTEGER NOT NULL,
                content_type TEXT NOT NULL,
                content TEXT NOT NULL,
                content_size INTEGER NOT NULL,
                owner_address TEXT,
                block_height INTEGER NOT NULL,
                block_time INTEGER NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_inscriptions_txid ON inscriptions(txid);
            CREATE INDEX IF NOT EXISTS idx_inscriptions_owner ON inscriptions(owner_address);
            CREATE INDEX IF NOT EXISTS idx_inscriptions_content_type ON inscriptions(content_type);
            CREATE INDEX IF NOT EXISTS idx_inscriptions_block_height ON inscriptions(block_height);
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Migration: Add current_location column if it doesn't exist (for existing databases)
        // SQLite doesn't support IF NOT EXISTS for ALTER TABLE, so we catch the error
        let column_added = sqlx::query(
            "ALTER TABLE inscriptions ADD COLUMN current_location TEXT"
        )
        .execute(&self.pool)
        .await
        .is_ok();
        
        if column_added {
            // Only create index if column was successfully added
            let _ = sqlx::query(
                "CREATE INDEX IF NOT EXISTS idx_inscriptions_current_location ON inscriptions(current_location)"
            )
            .execute(&self.pool)
            .await;
            
            // Migration: Update existing inscriptions to set current_location = inscription_id
            // This initializes current_location for inscriptions created before this feature
            let _ = sqlx::query(
                "UPDATE inscriptions SET current_location = inscription_id WHERE current_location IS NULL OR current_location = ''"
            )
            .execute(&self.pool)
            .await;
        }

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS dar20_tokens (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tick TEXT UNIQUE NOT NULL,
                max_supply TEXT NOT NULL,
                mint_limit TEXT NOT NULL,
                decimals INTEGER NOT NULL DEFAULT 18,
                total_minted TEXT NOT NULL DEFAULT '0',
                deploy_txid TEXT NOT NULL,
                deploy_address TEXT NOT NULL,
                block_height INTEGER NOT NULL,
                block_time INTEGER NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_tokens_tick ON dar20_tokens(tick);
            CREATE INDEX IF NOT EXISTS idx_tokens_deploy_address ON dar20_tokens(deploy_address);
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS dar20_mints (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tick TEXT NOT NULL,
                amount TEXT NOT NULL,
                to_address TEXT NOT NULL,
                txid TEXT NOT NULL,
                block_height INTEGER NOT NULL,
                block_time INTEGER NOT NULL,
                valid INTEGER NOT NULL DEFAULT 1,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_mints_tick ON dar20_mints(tick);
            CREATE INDEX IF NOT EXISTS idx_mints_to_address ON dar20_mints(to_address);
            CREATE INDEX IF NOT EXISTS idx_mints_txid ON dar20_mints(txid);
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS dar20_transfers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tick TEXT NOT NULL,
                amount TEXT NOT NULL,
                from_address TEXT NOT NULL,
                to_address TEXT NOT NULL,
                txid TEXT NOT NULL,
                block_height INTEGER NOT NULL,
                block_time INTEGER NOT NULL,
                valid INTEGER NOT NULL DEFAULT 1,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_transfers_tick ON dar20_transfers(tick);
            CREATE INDEX IF NOT EXISTS idx_transfers_from ON dar20_transfers(from_address);
            CREATE INDEX IF NOT EXISTS idx_transfers_to ON dar20_transfers(to_address);
            CREATE INDEX IF NOT EXISTS idx_transfers_txid ON dar20_transfers(txid);
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS dar20_balances (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tick TEXT NOT NULL,
                address TEXT NOT NULL,
                balance TEXT NOT NULL DEFAULT '0',
                locked_balance TEXT NOT NULL DEFAULT '0',
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(tick, address)
            );

            CREATE INDEX IF NOT EXISTS idx_balances_tick ON dar20_balances(tick);
            CREATE INDEX IF NOT EXISTS idx_balances_address ON dar20_balances(address);
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Migration: Add locked_balance column if it doesn't exist
        let locked_balance_added = sqlx::query(
            "ALTER TABLE dar20_balances ADD COLUMN locked_balance TEXT NOT NULL DEFAULT '0'"
        )
        .execute(&self.pool)
        .await
        .is_ok();
        
        if locked_balance_added {
            info!("Added locked_balance column to dar20_balances table");
        }

        // DashMap tables (like Bitmap)
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS dashmap_claims (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                block_height INTEGER UNIQUE NOT NULL,
                owner_address TEXT NOT NULL,
                inscription_id TEXT NOT NULL,
                claim_txid TEXT NOT NULL,
                claim_block_height INTEGER NOT NULL,
                claim_time INTEGER NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_dashmap_owner ON dashmap_claims(owner_address);
            CREATE INDEX IF NOT EXISTS idx_dashmap_inscription ON dashmap_claims(inscription_id);
            "#,
        )
        .execute(&self.pool)
        .await?;

        // DashDomain tables (like bitdomain)
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS dash_domains (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT UNIQUE NOT NULL,
                full_name TEXT UNIQUE NOT NULL,
                owner_address TEXT NOT NULL,
                inscription_id TEXT NOT NULL,
                registration_txid TEXT NOT NULL,
                registration_block INTEGER NOT NULL,
                registration_time INTEGER NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_domains_owner ON dash_domains(owner_address);
            CREATE INDEX IF NOT EXISTS idx_domains_inscription ON dash_domains(inscription_id);
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Migration: Add current_location column to dashmap_claims if it doesn't exist
        let dashmap_column_added = sqlx::query(
            "ALTER TABLE dashmap_claims ADD COLUMN current_location TEXT"
        )
        .execute(&self.pool)
        .await
        .is_ok();
        
        if dashmap_column_added {
            let _ = sqlx::query(
                "CREATE INDEX IF NOT EXISTS idx_dashmap_current_location ON dashmap_claims(current_location)"
            )
            .execute(&self.pool)
            .await;
            
            // Migration: Update existing dashmap_claims to set current_location = claim_txid:0
            let _ = sqlx::query(
                "UPDATE dashmap_claims SET current_location = claim_txid || ':0' WHERE current_location IS NULL OR current_location = ''"
            )
            .execute(&self.pool)
            .await;
        }

        // Migration: Add current_location column to dash_domains if it doesn't exist
        let domain_column_added = sqlx::query(
            "ALTER TABLE dash_domains ADD COLUMN current_location TEXT"
        )
        .execute(&self.pool)
        .await
        .is_ok();
        
        if domain_column_added {
            let _ = sqlx::query(
                "CREATE INDEX IF NOT EXISTS idx_domains_current_location ON dash_domains(current_location)"
            )
            .execute(&self.pool)
            .await;
            
            // Migration: Update existing dash_domains to set current_location = registration_txid:0
            let _ = sqlx::query(
                "UPDATE dash_domains SET current_location = registration_txid || ':0' WHERE current_location IS NULL OR current_location = ''"
            )
            .execute(&self.pool)
            .await;
        }

        info!("Database migrations completed");
        Ok(())
    }

    /// Get last indexed block (sync version for startup)
    pub fn get_last_indexed_block_sync(&self) -> u64 {
        // Return 0 as default, actual value will be fetched async
        0
    }

    /// Get last indexed block
    pub async fn get_last_indexed_block(&self) -> Result<u64> {
        let row: (i64,) = sqlx::query_as("SELECT last_indexed_block FROM indexer_state WHERE id = 1")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0 as u64)
    }

    /// Update last indexed block
    pub async fn set_last_indexed_block(&self, height: u64) -> Result<()> {
        sqlx::query("UPDATE indexer_state SET last_indexed_block = ?, updated_at = CURRENT_TIMESTAMP WHERE id = 1")
            .bind(height as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Clear data from a specific block height
    pub async fn clear_from_block(&self, height: u64) -> Result<()> {
        let height = height as i64;

        sqlx::query("DELETE FROM inscriptions WHERE block_height >= ?")
            .bind(height)
            .execute(&self.pool)
            .await?;

        sqlx::query("DELETE FROM dar20_mints WHERE block_height >= ?")
            .bind(height)
            .execute(&self.pool)
            .await?;

        sqlx::query("DELETE FROM dar20_transfers WHERE block_height >= ?")
            .bind(height)
            .execute(&self.pool)
            .await?;

        sqlx::query("DELETE FROM dar20_tokens WHERE block_height >= ?")
            .bind(height)
            .execute(&self.pool)
            .await?;

        // Recalculate all balances (simple approach - in production, optimize this)
        sqlx::query("DELETE FROM dar20_balances")
            .execute(&self.pool)
            .await?;

        self.set_last_indexed_block(height.saturating_sub(1) as u64)
            .await?;

        Ok(())
    }

    /// Get indexer statistics
    pub async fn get_stats(&self) -> Result<IndexerStats> {
        let inscriptions: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM inscriptions")
                .fetch_one(&self.pool)
                .await?;

        let tokens: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dar20_tokens")
            .fetch_one(&self.pool)
            .await?;

        let holders: (i64,) =
            sqlx::query_as("SELECT COUNT(DISTINCT address) FROM dar20_balances WHERE balance != '0'")
                .fetch_one(&self.pool)
                .await?;

        let mints: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dar20_mints WHERE valid = 1")
            .fetch_one(&self.pool)
            .await?;

        let transfers: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM dar20_transfers WHERE valid = 1")
                .fetch_one(&self.pool)
                .await?;

        let dashmap_claims: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM dashmap_claims")
                .fetch_one(&self.pool)
                .await
                .unwrap_or((0,));

        let domains: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM dash_domains")
                .fetch_one(&self.pool)
                .await
                .unwrap_or((0,));

        let last_block = self.get_last_indexed_block().await?;

        Ok(IndexerStats {
            total_inscriptions: inscriptions.0 as u64,
            total_tokens: tokens.0 as u64,
            total_holders: holders.0 as u64,
            total_mints: mints.0 as u64,
            total_transfers: transfers.0 as u64,
            total_dashmap_claims: dashmap_claims.0 as u64,
            total_domains: domains.0 as u64,
            last_indexed_block: last_block,
        })
    }

    // ==================== Inscription Methods ====================

    /// Insert inscription
    pub async fn insert_inscription(&self, inscription: &Inscription) -> Result<i64> {
        // Set current_location to inscription_id (creation location) if not set
        let current_location = inscription.current_location.as_ref().unwrap_or(&inscription.inscription_id);
        
        let result = sqlx::query(
            r#"
            INSERT INTO inscriptions (inscription_id, txid, vout, content_type, content, content_size, owner_address, current_location, block_height, block_time)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&inscription.inscription_id)
        .bind(&inscription.txid)
        .bind(inscription.vout)
        .bind(&inscription.content_type)
        .bind(&inscription.content)
        .bind(inscription.content_size)
        .bind(&inscription.owner_address)
        .bind(current_location)
        .bind(inscription.block_height as i64)
        .bind(inscription.block_time as i64)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    /// Get inscription by ID
    pub async fn get_inscription(&self, inscription_id: &str) -> Result<Option<Inscription>> {
        let row = sqlx::query_as::<_, (i64, String, String, i64, String, String, i64, Option<String>, Option<String>, i64, i64)>(
            "SELECT id, inscription_id, txid, vout, content_type, content, content_size, owner_address, current_location, block_height, block_time FROM inscriptions WHERE inscription_id = ?"
        )
        .bind(inscription_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| Inscription {
            id: Some(r.0),
            inscription_id: r.1,
            txid: r.2,
            vout: r.3 as u32,
            content_type: r.4,
            content: r.5,
            content_size: r.6 as u32,
            owner_address: r.7,
            current_location: r.8,
            block_height: r.9 as u64,
            block_time: r.10 as u64,
            created_at: None,
        }))
    }

    /// Update inscription content (for fixing truncated content)
    pub async fn update_inscription_content(&self, inscription_id: &str, content: &str, content_size: u32) -> Result<()> {
        sqlx::query(
            "UPDATE inscriptions SET content = ?, content_size = ? WHERE inscription_id = ?"
        )
        .bind(content)
        .bind(content_size as i64)
        .bind(inscription_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Find inscription by current location (UTXO)
    pub async fn get_inscription_by_location(&self, location: &str) -> Result<Option<Inscription>> {
        let row = sqlx::query_as::<_, (i64, String, String, i64, String, String, i64, Option<String>, Option<String>, i64, i64)>(
            "SELECT id, inscription_id, txid, vout, content_type, content, content_size, owner_address, current_location, block_height, block_time FROM inscriptions WHERE current_location = ?"
        )
        .bind(location)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| Inscription {
            id: Some(r.0),
            inscription_id: r.1,
            txid: r.2,
            vout: r.3 as u32,
            content_type: r.4,
            content: r.5,
            content_size: r.6 as u32,
            owner_address: r.7,
            current_location: r.8,
            block_height: r.9 as u64,
            block_time: r.10 as u64,
            created_at: None,
        }))
    }

    /// Update inscription location and owner (when transferred)
    pub async fn update_inscription_location(&self, inscription_id: &str, new_location: &str, new_owner: &str) -> Result<()> {
        sqlx::query(
            "UPDATE inscriptions SET current_location = ?, owner_address = ? WHERE inscription_id = ?"
        )
        .bind(new_location)
        .bind(new_owner)
        .bind(inscription_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get inscriptions by owner
    pub async fn get_inscriptions_by_owner(
        &self,
        address: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Inscription>> {
        let rows = sqlx::query_as::<_, (i64, String, String, i64, String, String, i64, Option<String>, Option<String>, i64, i64)>(
            "SELECT id, inscription_id, txid, vout, content_type, content, content_size, owner_address, current_location, block_height, block_time FROM inscriptions WHERE owner_address = ? ORDER BY block_height DESC LIMIT ? OFFSET ?"
        )
        .bind(address)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| Inscription {
                id: Some(r.0),
                inscription_id: r.1,
                txid: r.2,
                vout: r.3 as u32,
                content_type: r.4,
                content: r.5,
                content_size: r.6 as u32,
                owner_address: r.7,
                current_location: r.8,
                block_height: r.9 as u64,
                block_time: r.10 as u64,
                created_at: None,
            })
            .collect())
    }

    /// Count total inscriptions
    pub async fn count_inscriptions(&self) -> Result<u64> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM inscriptions")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0 as u64)
    }

    /// Count inscriptions by owner
    pub async fn count_inscriptions_by_owner(&self, address: &str) -> Result<u64> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM inscriptions WHERE owner_address = ?"
        )
        .bind(address)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0 as u64)
    }

    /// Count inscriptions by content type
    pub async fn count_inscriptions_by_type(&self, content_type: &str) -> Result<u64> {
        let pattern = format!("%{}%", content_type);
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM inscriptions WHERE content_type LIKE ?"
        )
        .bind(&pattern)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0 as u64)
    }

    /// Get inscriptions with pagination
    pub async fn get_inscriptions_paginated(&self, limit: u32, offset: u32, sort_desc: bool) -> Result<Vec<Inscription>> {
        let query = if sort_desc {
            "SELECT id, inscription_id, txid, vout, content_type, content, content_size, owner_address, current_location, block_height, block_time FROM inscriptions ORDER BY block_height DESC, id DESC LIMIT ? OFFSET ?"
        } else {
            "SELECT id, inscription_id, txid, vout, content_type, content, content_size, owner_address, current_location, block_height, block_time FROM inscriptions ORDER BY block_height ASC, id ASC LIMIT ? OFFSET ?"
        };

        let rows = sqlx::query_as::<_, (i64, String, String, i64, String, String, i64, Option<String>, Option<String>, i64, i64)>(query)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| Inscription {
                id: Some(r.0),
                inscription_id: r.1,
                txid: r.2,
                vout: r.3 as u32,
                content_type: r.4,
                content: r.5,
                content_size: r.6 as u32,
                owner_address: r.7,
                current_location: r.8,
                block_height: r.9 as u64,
                block_time: r.10 as u64,
                created_at: None,
            })
            .collect())
    }

    /// Get inscriptions filtered by content type with pagination
    pub async fn get_inscriptions_by_type(&self, content_type: &str, limit: u32, offset: u32, sort_desc: bool) -> Result<Vec<Inscription>> {
        let query = if sort_desc {
            "SELECT id, inscription_id, txid, vout, content_type, content, content_size, owner_address, current_location, block_height, block_time FROM inscriptions WHERE content_type LIKE ? ORDER BY block_height DESC, id DESC LIMIT ? OFFSET ?"
        } else {
            "SELECT id, inscription_id, txid, vout, content_type, content, content_size, owner_address, current_location, block_height, block_time FROM inscriptions WHERE content_type LIKE ? ORDER BY block_height ASC, id ASC LIMIT ? OFFSET ?"
        };

        let pattern = format!("%{}%", content_type);
        let rows = sqlx::query_as::<_, (i64, String, String, i64, String, String, i64, Option<String>, Option<String>, i64, i64)>(query)
            .bind(&pattern)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| Inscription {
                id: Some(r.0),
                inscription_id: r.1,
                txid: r.2,
                vout: r.3 as u32,
                content_type: r.4,
                content: r.5,
                content_size: r.6 as u32,
                owner_address: r.7,
                current_location: r.8,
                block_height: r.9 as u64,
                block_time: r.10 as u64,
                created_at: None,
            })
            .collect())
    }

    /// Get recent inscriptions (legacy - for backward compatibility)
    pub async fn get_recent_inscriptions(&self, limit: u32) -> Result<Vec<Inscription>> {
        self.get_inscriptions_paginated(limit, 0, true).await
    }

    // ==================== DAR-20 Token Methods ====================

    /// Insert DAR-20 token
    pub async fn insert_token(&self, token: &Dar20Token) -> Result<i64> {
        let result = sqlx::query(
            r#"
            INSERT INTO dar20_tokens (tick, max_supply, mint_limit, decimals, total_minted, deploy_txid, deploy_address, block_height, block_time)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&token.tick)
        .bind(&token.max_supply)
        .bind(&token.mint_limit)
        .bind(token.decimals)
        .bind(&token.total_minted)
        .bind(&token.deploy_txid)
        .bind(&token.deploy_address)
        .bind(token.block_height as i64)
        .bind(token.block_time as i64)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    /// Count total tokens
    pub async fn count_tokens(&self) -> Result<u64> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dar20_tokens")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0 as u64)
    }

    /// Get DAR-20 token by ticker
    pub async fn get_token(&self, tick: &str) -> Result<Option<Dar20Token>> {
        let row = sqlx::query_as::<_, (i64, String, String, String, i64, String, String, String, i64, i64)>(
            "SELECT id, tick, max_supply, mint_limit, decimals, total_minted, deploy_txid, deploy_address, block_height, block_time FROM dar20_tokens WHERE tick = ?"
        )
        .bind(tick.to_uppercase())
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| Dar20Token {
            id: Some(r.0),
            tick: r.1,
            max_supply: r.2,
            mint_limit: r.3,
            decimals: r.4 as u8,
            total_minted: r.5,
            deploy_txid: r.6,
            deploy_address: r.7,
            block_height: r.8 as u64,
            block_time: r.9 as u64,
            created_at: None,
        }))
    }

    /// Get all DAR-20 tokens (legacy)
    pub async fn get_all_tokens(&self) -> Result<Vec<Dar20Token>> {
        self.get_tokens_paginated(1000, 0, "deploy").await
    }

    /// Get tokens with pagination and sorting
    pub async fn get_tokens_paginated(&self, limit: u32, offset: u32, sort: &str) -> Result<Vec<Dar20Token>> {
        let order_by = match sort {
            "minted" => "CAST(total_minted AS REAL) DESC",
            "holders" => "block_height DESC", // Will be sorted in API after getting holder counts
            _ => "block_height DESC", // "deploy" - newest first
        };

        let query = format!(
            "SELECT id, tick, max_supply, mint_limit, decimals, total_minted, deploy_txid, deploy_address, block_height, block_time FROM dar20_tokens ORDER BY {} LIMIT ? OFFSET ?",
            order_by
        );

        let rows = sqlx::query_as::<_, (i64, String, String, String, i64, String, String, String, i64, i64)>(&query)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| Dar20Token {
                id: Some(r.0),
                tick: r.1,
                max_supply: r.2,
                mint_limit: r.3,
                decimals: r.4 as u8,
                total_minted: r.5,
                deploy_txid: r.6,
                deploy_address: r.7,
                block_height: r.8 as u64,
                block_time: r.9 as u64,
                created_at: None,
            })
            .collect())
    }

    /// Update token total minted
    pub async fn update_token_minted(&self, tick: &str, total_minted: &str) -> Result<()> {
        sqlx::query("UPDATE dar20_tokens SET total_minted = ? WHERE tick = ?")
            .bind(total_minted)
            .bind(tick.to_uppercase())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ==================== DAR-20 Mint Methods ====================

    /// Insert DAR-20 mint
    pub async fn insert_mint(&self, mint: &Dar20Mint) -> Result<i64> {
        let result = sqlx::query(
            r#"
            INSERT INTO dar20_mints (tick, amount, to_address, txid, block_height, block_time, valid)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&mint.tick)
        .bind(&mint.amount)
        .bind(&mint.to_address)
        .bind(&mint.txid)
        .bind(mint.block_height as i64)
        .bind(mint.block_time as i64)
        .bind(mint.valid)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    /// Count mints for a token
    pub async fn count_token_mints(&self, tick: &str) -> Result<u64> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dar20_mints WHERE tick = ?")
            .bind(tick.to_uppercase())
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0 as u64)
    }

    /// Count mints with filters
    pub async fn count_token_mints_filtered(&self, tick: &str, address: Option<&str>, valid: Option<bool>) -> Result<u64> {
        let mut query = "SELECT COUNT(*) FROM dar20_mints WHERE tick = ?".to_string();
        
        if address.is_some() {
            query.push_str(" AND to_address = ?");
        }
        if let Some(v) = valid {
            query.push_str(&format!(" AND valid = {}", if v { 1 } else { 0 }));
        }

        let mut q = sqlx::query_as::<_, (i64,)>(&query).bind(tick.to_uppercase());
        
        if let Some(addr) = address {
            q = q.bind(addr);
        }

        let row = q.fetch_one(&self.pool).await?;
        Ok(row.0 as u64)
    }

    /// Get mints for a token (legacy)
    pub async fn get_mints_by_token(&self, tick: &str, limit: u32) -> Result<Vec<Dar20Mint>> {
        self.get_mints_paginated(tick, limit, 0, None, None).await
    }

    /// Get mints with pagination and filters
    pub async fn get_mints_paginated(
        &self,
        tick: &str,
        limit: u32,
        offset: u32,
        address: Option<&str>,
        valid: Option<bool>,
    ) -> Result<Vec<Dar20Mint>> {
        let mut query = "SELECT id, tick, amount, to_address, txid, block_height, block_time, valid FROM dar20_mints WHERE tick = ?".to_string();
        
        if address.is_some() {
            query.push_str(" AND to_address = ?");
        }
        if let Some(v) = valid {
            query.push_str(&format!(" AND valid = {}", if v { 1 } else { 0 }));
        }
        
        query.push_str(" ORDER BY block_height DESC LIMIT ? OFFSET ?");

        let mut q = sqlx::query_as::<_, (i64, String, String, String, String, i64, i64, bool)>(&query)
            .bind(tick.to_uppercase());
        
        if let Some(addr) = address {
            q = q.bind(addr);
        }
        
        let rows = q.bind(limit).bind(offset).fetch_all(&self.pool).await?;

        Ok(rows
            .into_iter()
            .map(|r| Dar20Mint {
                id: Some(r.0),
                tick: r.1,
                amount: r.2,
                to_address: r.3,
                txid: r.4,
                block_height: r.5 as u64,
                block_time: r.6 as u64,
                valid: r.7,
                created_at: None,
            })
            .collect())
    }

    // ==================== DAR-20 Transfer Methods ====================

    /// Insert DAR-20 transfer
    pub async fn insert_transfer(&self, transfer: &Dar20Transfer) -> Result<i64> {
        let result = sqlx::query(
            r#"
            INSERT INTO dar20_transfers (tick, amount, from_address, to_address, txid, block_height, block_time, valid)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&transfer.tick)
        .bind(&transfer.amount)
        .bind(&transfer.from_address)
        .bind(&transfer.to_address)
        .bind(&transfer.txid)
        .bind(transfer.block_height as i64)
        .bind(transfer.block_time as i64)
        .bind(transfer.valid)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    /// Get DAR-20 transfer by txid
    pub async fn get_transfer_by_txid(&self, txid: &str) -> Result<Option<Dar20Transfer>> {
        let row = sqlx::query_as::<_, (i64, String, String, String, String, String, i64, i64, bool)>(
            "SELECT id, tick, amount, from_address, to_address, txid, block_height, block_time, valid FROM dar20_transfers WHERE txid = ?"
        )
        .bind(txid)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| Dar20Transfer {
            id: Some(r.0),
            tick: r.1,
            amount: r.2,
            from_address: r.3,
            to_address: r.4,
            txid: r.5,
            block_height: r.6 as u64,
            block_time: r.7 as u64,
            valid: r.8,
            created_at: None,
        }))
    }

    /// Update DAR-20 transfer recipient when transfer is executed
    pub async fn update_transfer_recipient(
        &self,
        inscription_txid: &str,
        recipient: &str,
        valid: bool,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE dar20_transfers
            SET to_address = ?, valid = ?
            WHERE txid = ? AND to_address = 'pending'
            "#,
        )
        .bind(recipient)
        .bind(valid)
        .bind(inscription_txid)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get transfers for a token with pagination and filters
    pub async fn get_transfers_paginated(
        &self,
        tick: &str,
        limit: u32,
        offset: u32,
        from_address: Option<&str>,
        to_address: Option<&str>,
        valid: Option<bool>,
    ) -> Result<Vec<Dar20Transfer>> {
        let mut query = "SELECT id, tick, amount, from_address, to_address, txid, block_height, block_time, valid FROM dar20_transfers WHERE tick = ?".to_string();
        
        if from_address.is_some() {
            query.push_str(" AND from_address = ?");
        }
        if to_address.is_some() {
            query.push_str(" AND to_address = ?");
        }
        if let Some(v) = valid {
            query.push_str(&format!(" AND valid = {}", if v { 1 } else { 0 }));
        }
        
        query.push_str(" ORDER BY block_height DESC LIMIT ? OFFSET ?");

        let mut q = sqlx::query_as::<_, (i64, String, String, String, String, String, i64, i64, bool)>(&query)
            .bind(tick.to_uppercase());
        
        if let Some(addr) = from_address {
            q = q.bind(addr);
        }
        if let Some(addr) = to_address {
            q = q.bind(addr);
        }
        
        let rows = q.bind(limit).bind(offset).fetch_all(&self.pool).await?;

        Ok(rows
            .into_iter()
            .map(|r| Dar20Transfer {
                id: Some(r.0),
                tick: r.1,
                amount: r.2,
                from_address: r.3,
                to_address: r.4,
                txid: r.5,
                block_height: r.6 as u64,
                block_time: r.7 as u64,
                valid: r.8,
                created_at: None,
            })
            .collect())
    }

    /// Count transfers for a token with filters
    pub async fn count_token_transfers_filtered(
        &self,
        tick: &str,
        from_address: Option<&str>,
        to_address: Option<&str>,
        valid: Option<bool>,
    ) -> Result<i64> {
        let mut query = "SELECT COUNT(*) FROM dar20_transfers WHERE tick = ?".to_string();
        
        if from_address.is_some() {
            query.push_str(" AND from_address = ?");
        }
        if to_address.is_some() {
            query.push_str(" AND to_address = ?");
        }
        if let Some(v) = valid {
            query.push_str(&format!(" AND valid = {}", if v { 1 } else { 0 }));
        }
        
        let mut q = sqlx::query_as::<_, (i64,)>(&query)
            .bind(tick.to_uppercase());
        
        if let Some(addr) = from_address {
            q = q.bind(addr);
        }
        if let Some(addr) = to_address {
            q = q.bind(addr);
        }
        
        let row = q.fetch_one(&self.pool).await?;
        Ok(row.0)
    }

    // ==================== DAR-20 Balance Methods ====================

    /// Get total balance (including locked) for address and token
    pub async fn get_total_balance(&self, tick: &str, address: &str) -> Result<String> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT balance FROM dar20_balances WHERE tick = ? AND address = ?",
        )
        .bind(tick.to_uppercase())
        .bind(address)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| r.0).unwrap_or_else(|| "0".to_string()))
    }

    /// Get locked balance for address and token
    pub async fn get_locked_balance(&self, tick: &str, address: &str) -> Result<String> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT COALESCE(locked_balance, '0') FROM dar20_balances WHERE tick = ? AND address = ?",
        )
        .bind(tick.to_uppercase())
        .bind(address)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| r.0).unwrap_or_else(|| "0".to_string()))
    }

    /// Get available balance (total - locked) for address and token
    /// This is the canonical BRC-20 behavior: available = balance - locked
    pub async fn get_balance(&self, tick: &str, address: &str) -> Result<String> {
        use crate::parser::{parse_amount, format_amount};
        
        let total = self.get_total_balance(tick, address).await?;
        let locked = self.get_locked_balance(tick, address).await?;
        
        // For backward compatibility, if we can't parse, return total
        // (old records won't have locked_balance)
        let token = match self.get_token(tick).await? {
            Some(t) => t,
            None => return Ok(total), // Can't calculate without decimals
        };
        
        let total_amount = parse_amount(&total, token.decimals).unwrap_or(0);
        let locked_amount = parse_amount(&locked, token.decimals).unwrap_or(0);
        let available = total_amount.saturating_sub(locked_amount);
        
        Ok(format_amount(available, token.decimals))
    }

    /// Update total balance for address and token (preserves locked_balance)
    pub async fn update_balance(&self, tick: &str, address: &str, balance: &str) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO dar20_balances (tick, address, balance, locked_balance, updated_at)
            VALUES (?, ?, ?, COALESCE((SELECT locked_balance FROM dar20_balances WHERE tick = ? AND address = ?), '0'), CURRENT_TIMESTAMP)
            ON CONFLICT(tick, address) DO UPDATE SET balance = ?, updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(tick.to_uppercase())
        .bind(address)
        .bind(balance)
        .bind(tick.to_uppercase())
        .bind(address)
        .bind(balance)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update locked balance for address and token
    pub async fn update_locked_balance(&self, tick: &str, address: &str, locked_balance: &str) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO dar20_balances (tick, address, balance, locked_balance, updated_at)
            VALUES (?, ?, COALESCE((SELECT balance FROM dar20_balances WHERE tick = ? AND address = ?), '0'), ?, CURRENT_TIMESTAMP)
            ON CONFLICT(tick, address) DO UPDATE SET locked_balance = ?, updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(tick.to_uppercase())
        .bind(address)
        .bind(tick.to_uppercase())
        .bind(address)
        .bind(locked_balance)
        .bind(locked_balance)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Lock tokens: add to locked_balance (used when transfer inscription is created)
    pub async fn lock_balance(&self, tick: &str, address: &str, amount: &str) -> Result<()> {
        use crate::parser::{parse_amount, format_amount};
        
        let token = match self.get_token(tick).await? {
            Some(t) => t,
            None => return Err(anyhow::anyhow!("Token {} not found", tick)),
        };
        
        let current_locked = self.get_locked_balance(tick, address).await?;
        let locked_amount = parse_amount(&current_locked, token.decimals).unwrap_or(0);
        let lock_amount = parse_amount(amount, token.decimals)
            .ok_or_else(|| anyhow::anyhow!("Invalid lock amount: {}", amount))?;
        
        let new_locked = locked_amount + lock_amount;
        self.update_locked_balance(tick, address, &format_amount(new_locked, token.decimals)).await?;
        
        Ok(())
    }

    /// Unlock tokens: subtract from locked_balance (used when transfer UTXO is spent)
    pub async fn unlock_balance(&self, tick: &str, address: &str, amount: &str) -> Result<()> {
        use crate::parser::{parse_amount, format_amount};
        
        let token = match self.get_token(tick).await? {
            Some(t) => t,
            None => return Err(anyhow::anyhow!("Token {} not found", tick)),
        };
        
        let current_locked = self.get_locked_balance(tick, address).await?;
        let locked_amount = parse_amount(&current_locked, token.decimals).unwrap_or(0);
        let unlock_amount = parse_amount(amount, token.decimals)
            .ok_or_else(|| anyhow::anyhow!("Invalid unlock amount: {}", amount))?;
        
        let new_locked = locked_amount.saturating_sub(unlock_amount);
        self.update_locked_balance(tick, address, &format_amount(new_locked, token.decimals)).await?;
        
        Ok(())
    }

    /// Get all balances for an address (with locked balance info)
    pub async fn get_balances_by_address(&self, address: &str) -> Result<Vec<Dar20Balance>> {
        use crate::parser::{parse_amount, format_amount};
        
        let rows = sqlx::query_as::<_, (i64, String, String, String, String)>(
            "SELECT id, tick, address, balance, COALESCE(locked_balance, '0') FROM dar20_balances WHERE address = ? AND (CAST(balance AS REAL) > 0 OR CAST(COALESCE(locked_balance, '0') AS REAL) > 0)"
        )
        .bind(address)
        .fetch_all(&self.pool)
        .await?;

        let mut balances = Vec::new();
        for row in rows {
            let (id, tick, address, total_balance, locked_balance) = row;
            
            // Get token decimals to calculate available balance
            let token = match self.get_token(&tick).await? {
                Some(t) => t,
                None => continue, // Skip if token not found
            };
            
            let total = parse_amount(&total_balance, token.decimals).unwrap_or(0);
            let locked = parse_amount(&locked_balance, token.decimals).unwrap_or(0);
            let available = total.saturating_sub(locked);
            
            balances.push(Dar20Balance {
                id: Some(id),
                tick,
                address,
                balance: format_amount(available, token.decimals), // Available balance
                total_balance: Some(total_balance.clone()),
                locked_balance: if locked > 0 { Some(locked_balance) } else { None },
                updated_at: None,
            });
        }
        
        Ok(balances)
    }

    /// Get all holders for a token
    pub async fn get_token_holders(&self, tick: &str, limit: u32) -> Result<Vec<Dar20Balance>> {
        self.get_token_holders_paginated(tick, limit, 0).await
    }

    /// Get token holders with pagination (sorted by total balance, showing available balance)
    pub async fn get_token_holders_paginated(&self, tick: &str, limit: u32, offset: u32) -> Result<Vec<Dar20Balance>> {
        use crate::parser::{parse_amount, format_amount};
        
        let token = match self.get_token(tick).await? {
            Some(t) => t,
            None => return Ok(Vec::new()),
        };
        
        let rows = sqlx::query_as::<_, (i64, String, String, String, String)>(
            "SELECT id, tick, address, balance, COALESCE(locked_balance, '0') FROM dar20_balances WHERE tick = ? AND CAST(balance AS REAL) > 0 ORDER BY CAST(balance AS REAL) DESC LIMIT ? OFFSET ?"
        )
        .bind(tick.to_uppercase())
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let (id, tick, address, total_balance, locked_balance) = r;
                let total = parse_amount(&total_balance, token.decimals).unwrap_or(0);
                let locked = parse_amount(&locked_balance, token.decimals).unwrap_or(0);
                let available = total.saturating_sub(locked);
                
                Dar20Balance {
                    id: Some(id),
                    tick,
                    address,
                    balance: format_amount(available, token.decimals), // Available balance
                    total_balance: Some(total_balance),
                    locked_balance: if locked > 0 { Some(locked_balance) } else { None },
                    updated_at: None,
                }
            })
            .collect())
    }

    /// Get holder rank (position by balance)
    pub async fn get_holder_rank(&self, tick: &str, address: &str) -> Result<u64> {
        let row: (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(*) + 1 FROM dar20_balances 
            WHERE tick = ? AND balance != '0' 
            AND CAST(balance AS REAL) > (
                SELECT COALESCE(CAST(balance AS REAL), 0) FROM dar20_balances WHERE tick = ? AND address = ?
            )
            "#
        )
        .bind(tick.to_uppercase())
        .bind(tick.to_uppercase())
        .bind(address)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0 as u64)
    }

    /// Count holders for a token
    pub async fn count_token_holders(&self, tick: &str) -> Result<u64> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM dar20_balances WHERE tick = ? AND balance != '0'",
        )
        .bind(tick.to_uppercase())
        .fetch_one(&self.pool)
        .await?;

        Ok(row.0 as u64)
    }

    // ==================== DashMap Methods ====================

    /// Check if a block is already claimed
    pub async fn is_block_claimed(&self, block_height: u64) -> Result<bool> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM dashmap_claims WHERE block_height = ?",
        )
        .bind(block_height as i64)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.is_some())
    }

    /// Insert DashMap claim
    pub async fn insert_dashmap_claim(&self, claim: &DashMapClaim) -> Result<i64> {
        // Set current_location to claim_txid:0 if not set
        let default_location = format!("{}:0", claim.claim_txid);
        let current_location = claim.current_location.as_ref()
            .unwrap_or(&default_location);
        
        let result = sqlx::query(
            r#"
            INSERT INTO dashmap_claims (block_height, owner_address, inscription_id, current_location, claim_txid, claim_block_height, claim_time)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(claim.block_height as i64)
        .bind(&claim.owner_address)
        .bind(&claim.inscription_id)
        .bind(current_location)
        .bind(&claim.claim_txid)
        .bind(claim.claim_block_height as i64)
        .bind(claim.claim_time as i64)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    /// Get DashMap claim by block height
    pub async fn get_dashmap_claim(&self, block_height: u64) -> Result<Option<DashMapClaim>> {
        let row = sqlx::query_as::<_, (i64, i64, String, String, Option<String>, String, i64, i64)>(
            "SELECT id, block_height, owner_address, inscription_id, current_location, claim_txid, claim_block_height, claim_time FROM dashmap_claims WHERE block_height = ?"
        )
        .bind(block_height as i64)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| DashMapClaim {
            id: Some(r.0),
            block_height: r.1 as u64,
            owner_address: r.2,
            inscription_id: r.3,
            current_location: r.4,
            claim_txid: r.5,
            claim_block_height: r.6 as u64,
            claim_time: r.7 as u64,
            created_at: None,
        }))
    }

    /// Get recent DashMap claims (legacy)
    pub async fn get_recent_dashmap_claims(&self, limit: u32) -> Result<Vec<DashMapClaim>> {
        self.get_dashmap_claims_paginated(limit, 0, "newest").await
    }

    /// Get DashMap claims with pagination and sorting
    pub async fn get_dashmap_claims_paginated(&self, limit: u32, offset: u32, sort: &str) -> Result<Vec<DashMapClaim>> {
        let order_by = match sort {
            "height" => "block_height DESC",
            _ => "claim_block_height DESC, id DESC", // "newest"
        };

        let query = format!(
            "SELECT id, block_height, owner_address, inscription_id, current_location, claim_txid, claim_block_height, claim_time FROM dashmap_claims ORDER BY {} LIMIT ? OFFSET ?",
            order_by
        );

        let rows = sqlx::query_as::<_, (i64, i64, String, String, Option<String>, String, i64, i64)>(&query)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| DashMapClaim {
                id: Some(r.0),
                block_height: r.1 as u64,
                owner_address: r.2,
                inscription_id: r.3,
                current_location: r.4,
                claim_txid: r.5,
                claim_block_height: r.6 as u64,
                claim_time: r.7 as u64,
                created_at: None,
            })
            .collect())
    }

    /// Count total DashMap claims
    pub async fn count_dashmap_claims(&self) -> Result<u64> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dashmap_claims")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0 as u64)
    }

    /// Count unique DashMap claimers
    pub async fn count_unique_dashmap_claimers(&self) -> Result<u64> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(DISTINCT owner_address) FROM dashmap_claims")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0 as u64)
    }

    /// Get top DashMap claimers by block count
    pub async fn get_top_dashmap_claimers(&self, limit: u32) -> Result<Vec<(String, u64)>> {
        let rows = sqlx::query_as::<_, (String, i64)>(
            "SELECT owner_address, COUNT(*) as cnt FROM dashmap_claims GROUP BY owner_address ORDER BY cnt DESC LIMIT ?"
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(addr, cnt)| (addr, cnt as u64)).collect())
    }

    /// Count DashMap claims by owner
    pub async fn count_dashmap_by_owner(&self, address: &str) -> Result<u64> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM dashmap_claims WHERE owner_address = ?",
        )
        .bind(address)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0 as u64)
    }

    /// Get DashMap claims by owner with pagination
    pub async fn get_dashmap_by_owner_paginated(&self, address: &str, limit: u32, offset: u32) -> Result<Vec<DashMapClaim>> {
        let rows = sqlx::query_as::<_, (i64, i64, String, String, Option<String>, String, i64, i64)>(
            "SELECT id, block_height, owner_address, inscription_id, current_location, claim_txid, claim_block_height, claim_time FROM dashmap_claims WHERE owner_address = ? ORDER BY block_height ASC LIMIT ? OFFSET ?"
        )
        .bind(address)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| DashMapClaim {
                id: Some(r.0),
                block_height: r.1 as u64,
                owner_address: r.2,
                inscription_id: r.3,
                current_location: r.4,
                claim_txid: r.5,
                claim_block_height: r.6 as u64,
                claim_time: r.7 as u64,
                created_at: None,
            })
            .collect())
    }

    /// Find DashMap claim by current location (UTXO)
    pub async fn get_dashmap_by_location(&self, location: &str) -> Result<Option<DashMapClaim>> {
        let row = sqlx::query_as::<_, (i64, i64, String, String, Option<String>, String, i64, i64)>(
            "SELECT id, block_height, owner_address, inscription_id, current_location, claim_txid, claim_block_height, claim_time FROM dashmap_claims WHERE current_location = ?"
        )
        .bind(location)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| DashMapClaim {
            id: Some(r.0),
            block_height: r.1 as u64,
            owner_address: r.2,
            inscription_id: r.3,
            current_location: r.4,
            claim_txid: r.5,
            claim_block_height: r.6 as u64,
            claim_time: r.7 as u64,
            created_at: None,
        }))
    }

    /// Update DashMap claim location and owner (when transferred)
    pub async fn update_dashmap_location(&self, block_height: u64, new_location: &str, new_owner: &str) -> Result<()> {
        sqlx::query(
            "UPDATE dashmap_claims SET current_location = ?, owner_address = ? WHERE block_height = ?"
        )
        .bind(new_location)
        .bind(new_owner)
        .bind(block_height as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // ==================== DashDomain Methods ====================

    /// Check if a domain is already registered
    pub async fn is_domain_registered(&self, name: &str) -> Result<bool> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM dash_domains WHERE name = ?",
        )
        .bind(name.to_lowercase())
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.is_some())
    }

    /// Insert DashDomain registration
    pub async fn insert_domain(&self, domain: &DashDomain) -> Result<i64> {
        // Set current_location to registration_txid:0 if not set
        let default_location = format!("{}:0", domain.registration_txid);
        let current_location = domain.current_location.as_ref()
            .unwrap_or(&default_location);
        
        let result = sqlx::query(
            r#"
            INSERT INTO dash_domains (name, full_name, owner_address, inscription_id, current_location, registration_txid, registration_block, registration_time)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&domain.name)
        .bind(&domain.full_name)
        .bind(&domain.owner_address)
        .bind(&domain.inscription_id)
        .bind(current_location)
        .bind(&domain.registration_txid)
        .bind(domain.registration_block as i64)
        .bind(domain.registration_time as i64)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    /// Get domain by name
    pub async fn get_domain(&self, name: &str) -> Result<Option<DashDomain>> {
        // Remove .dash suffix if present
        let name_clean = name.to_lowercase().trim_end_matches(".dash").to_string();
        
        let row = sqlx::query_as::<_, (i64, String, String, String, String, Option<String>, String, i64, i64)>(
            "SELECT id, name, full_name, owner_address, inscription_id, current_location, registration_txid, registration_block, registration_time FROM dash_domains WHERE name = ?"
        )
        .bind(&name_clean)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| DashDomain {
            id: Some(r.0),
            name: r.1,
            full_name: r.2,
            owner_address: r.3,
            inscription_id: r.4,
            current_location: r.5,
            registration_txid: r.6,
            registration_block: r.7 as u64,
            registration_time: r.8 as u64,
            created_at: None,
        }))
    }

    /// Get recent domains (legacy)
    pub async fn get_recent_domains(&self, limit: u32) -> Result<Vec<DashDomain>> {
        self.get_domains_paginated(limit, 0, "newest").await
    }

    /// Get domains with pagination and sorting
    pub async fn get_domains_paginated(&self, limit: u32, offset: u32, sort: &str) -> Result<Vec<DashDomain>> {
        let order_by = match sort {
            "name" => "name ASC",
            _ => "registration_block DESC, id DESC", // "newest"
        };

        let query = format!(
            "SELECT id, name, full_name, owner_address, inscription_id, current_location, registration_txid, registration_block, registration_time FROM dash_domains ORDER BY {} LIMIT ? OFFSET ?",
            order_by
        );

        let rows = sqlx::query_as::<_, (i64, String, String, String, String, Option<String>, String, i64, i64)>(&query)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| DashDomain {
                id: Some(r.0),
                name: r.1,
                full_name: r.2,
                owner_address: r.3,
                inscription_id: r.4,
                current_location: r.5,
                registration_txid: r.6,
                registration_block: r.7 as u64,
                registration_time: r.8 as u64,
                created_at: None,
            })
            .collect())
    }

    /// Count total domains
    pub async fn count_domains(&self) -> Result<u64> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dash_domains")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0 as u64)
    }

    /// Count unique domain owners
    pub async fn count_unique_domain_owners(&self) -> Result<u64> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(DISTINCT owner_address) FROM dash_domains")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0 as u64)
    }

    /// Get top domain owners
    pub async fn get_top_domain_owners(&self, limit: u32) -> Result<Vec<(String, u64)>> {
        let rows = sqlx::query_as::<_, (String, i64)>(
            "SELECT owner_address, COUNT(*) as cnt FROM dash_domains GROUP BY owner_address ORDER BY cnt DESC LIMIT ?"
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(addr, cnt)| (addr, cnt as u64)).collect())
    }

    /// Count domains by owner
    pub async fn count_domains_by_owner(&self, address: &str) -> Result<u64> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM dash_domains WHERE owner_address = ?",
        )
        .bind(address)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0 as u64)
    }

    /// Get domains by owner with pagination
    pub async fn get_domains_by_owner_paginated(&self, address: &str, limit: u32, offset: u32) -> Result<Vec<DashDomain>> {
        let rows = sqlx::query_as::<_, (i64, String, String, String, String, Option<String>, String, i64, i64)>(
            "SELECT id, name, full_name, owner_address, inscription_id, current_location, registration_txid, registration_block, registration_time FROM dash_domains WHERE owner_address = ? ORDER BY name ASC LIMIT ? OFFSET ?"
        )
        .bind(address)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| DashDomain {
                id: Some(r.0),
                name: r.1,
                full_name: r.2,
                owner_address: r.3,
                inscription_id: r.4,
                current_location: r.5,
                registration_txid: r.6,
                registration_block: r.7 as u64,
                registration_time: r.8 as u64,
                created_at: None,
            })
            .collect())
    }

    /// Count domains matching search
    pub async fn count_domains_search(&self, prefix: &str) -> Result<u64> {
        let search_pattern = format!("{}%", prefix.to_lowercase());
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dash_domains WHERE name LIKE ?")
            .bind(&search_pattern)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0 as u64)
    }

    /// Search domains by prefix (legacy)
    pub async fn search_domains(&self, prefix: &str, limit: u32) -> Result<Vec<DashDomain>> {
        self.search_domains_paginated(prefix, limit, 0).await
    }

    /// Search domains with pagination
    pub async fn search_domains_paginated(&self, prefix: &str, limit: u32, offset: u32) -> Result<Vec<DashDomain>> {
        let search_pattern = format!("{}%", prefix.to_lowercase());
        
        let rows = sqlx::query_as::<_, (i64, String, String, String, String, Option<String>, String, i64, i64)>(
            "SELECT id, name, full_name, owner_address, inscription_id, current_location, registration_txid, registration_block, registration_time FROM dash_domains WHERE name LIKE ? ORDER BY name ASC LIMIT ? OFFSET ?"
        )
        .bind(&search_pattern)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| DashDomain {
                id: Some(r.0),
                name: r.1,
                full_name: r.2,
                owner_address: r.3,
                inscription_id: r.4,
                current_location: r.5,
                registration_txid: r.6,
                registration_block: r.7 as u64,
                registration_time: r.8 as u64,
                created_at: None,
            })
            .collect())
    }

    /// Find DashDomain by current location (UTXO)
    pub async fn get_domain_by_location(&self, location: &str) -> Result<Option<DashDomain>> {
        let row = sqlx::query_as::<_, (i64, String, String, String, String, Option<String>, String, i64, i64)>(
            "SELECT id, name, full_name, owner_address, inscription_id, current_location, registration_txid, registration_block, registration_time FROM dash_domains WHERE current_location = ?"
        )
        .bind(location)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| DashDomain {
            id: Some(r.0),
            name: r.1,
            full_name: r.2,
            owner_address: r.3,
            inscription_id: r.4,
            current_location: r.5,
            registration_txid: r.6,
            registration_block: r.7 as u64,
            registration_time: r.8 as u64,
            created_at: None,
        }))
    }

    /// Update DashDomain location and owner (when transferred)
    pub async fn update_domain_location(&self, name: &str, new_location: &str, new_owner: &str) -> Result<()> {
        sqlx::query(
            "UPDATE dash_domains SET current_location = ?, owner_address = ? WHERE name = ?"
        )
        .bind(new_location)
        .bind(new_owner)
        .bind(name)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

