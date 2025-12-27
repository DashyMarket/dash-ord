use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

use crate::db::Database;
use crate::models::{ApiResponse, DashDomain, DashMapClaim, IndexerStats, Inscription, PaginatedResponse, Pagination};

/// Application state
#[derive(Clone)]
pub struct AppState {
    db: Database,
}

/// Start the API server
pub async fn start_server(db: Database, port: u16) -> anyhow::Result<()> {
    let state = Arc::new(AppState { db });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        // Health & Status
        .route("/", get(root))
        .route("/health", get(health))
        .route("/status", get(status))
        // Inscriptions
        .route("/inscriptions", get(get_inscriptions))
        .route("/inscriptions/:id", get(get_inscription))
        .route("/inscriptions/address/:address", get(get_inscriptions_by_address))
        // DAR-20 Tokens
        .route("/dar20/tokens", get(get_tokens))
        // More specific routes must come before generic :tick route
        .route("/dar20/tokens/:tick/holders", get(get_token_holders))
        .route("/dar20/tokens/:tick/mints", get(get_token_mints))
        .route("/dar20/tokens/:tick/transfers", get(get_token_transfers))
        .route("/dar20/tokens/:tick", get(get_token))
        // DAR-20 Balances
        .route("/dar20/balances/:address", get(get_balances_by_address))
        .route("/dar20/balance/:tick/:address", get(get_balance))
        // DashMap (like Bitmap)
        .route("/dashmap", get(get_dashmap_claims))
        .route("/dashmap/block/:height", get(get_dashmap_block))
        .route("/dashmap/address/:address", get(get_dashmap_by_address))
        .route("/dashmap/stats", get(get_dashmap_stats))
        // DashDomain (like bitdomain)
        .route("/domains", get(get_domains))
        .route("/domains/search", get(search_domains))
        .route("/domains/stats", get(get_domain_stats))
        .route("/domains/name/:name", get(get_domain))
        .route("/domains/address/:address", get(get_domains_by_address))
        .layer(cors)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    info!("API server listening on http://0.0.0.0:{}", port);

    axum::serve(listener, app).await?;

    Ok(())
}

// ==================== Health & Status ====================

async fn root() -> impl IntoResponse {
    Json(serde_json::json!({
        "name": "Dash Indexer API",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Indexer for Dash Ordinals, DAR-20 tokens, DashMap, and DashDomain",
        "endpoints": {
            "health": {
                "GET /health": "Health check with DB and RPC status"
            },
            "status": {
                "GET /status": "Indexer statistics and sync progress"
            },
            "inscriptions": {
                "GET /inscriptions": "List inscriptions (paginated)",
                "GET /inscriptions/:id": "Get inscription by ID",
                "GET /inscriptions/address/:address": "Get inscriptions by owner"
            },
            "dar20": {
                "GET /dar20/tokens": "List all DAR-20 tokens",
                "GET /dar20/tokens/:tick": "Get token details",
                "GET /dar20/tokens/:tick/holders": "Get token holders",
                "GET /dar20/tokens/:tick/mints": "Get mint history",
                "GET /dar20/tokens/:tick/transfers": "Get transfer history",
                "GET /dar20/balances/:address": "Get balances by address",
                "GET /dar20/balance/:tick/:address": "Get specific balance"
            },
            "dashmap": {
                "GET /dashmap": "List block claims",
                "GET /dashmap/stats": "DashMap statistics",
                "GET /dashmap/block/:height": "Check block claim status",
                "GET /dashmap/address/:address": "Get blocks by owner"
            },
            "domains": {
                "GET /domains": "List domains",
                "GET /domains/stats": "Domain statistics",
                "GET /domains/name/:name": "Check domain availability",
                "GET /domains/address/:address": "Get domains by owner",
                "GET /domains/search?q=xxx": "Search domains"
            }
        }
    }))
}

#[derive(Debug, Serialize)]
pub struct HealthStatus {
    pub status: String,
    pub database: ServiceHealth,
    pub timestamp: u64,
    pub uptime_seconds: u64,
}

#[derive(Debug, Serialize)]
pub struct ServiceHealth {
    pub status: String,
    pub latency_ms: u64,
}

static START_TIME: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn get_uptime() -> u64 {
    START_TIME
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs()
}

async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let start = std::time::Instant::now();
    
    // Check database connectivity
    let db_health = match state.db.health_check().await {
        Ok(_) => ServiceHealth {
            status: "ok".to_string(),
            latency_ms: start.elapsed().as_millis() as u64,
        },
        Err(e) => ServiceHealth {
            status: format!("error: {}", e),
            latency_ms: start.elapsed().as_millis() as u64,
        },
    };
    
    let overall_status = if db_health.status == "ok" { "healthy" } else { "unhealthy" };
    
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let health = HealthStatus {
        status: overall_status.to_string(),
        database: db_health,
        timestamp,
        uptime_seconds: get_uptime(),
    };
    
    Json(ApiResponse::success(health))
}

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub version: String,
    pub uptime_seconds: u64,
    pub sync: SyncStatus,
    pub stats: IndexerStats,
}

#[derive(Debug, Serialize)]
pub struct SyncStatus {
    pub indexed_blocks: u64,
    pub chain_height: u64,
    pub sync_percentage: f64,
    pub is_synced: bool,
    pub blocks_behind: u64,
}

async fn status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.db.get_stats().await {
        Ok(stats) => {
            // Get chain height from last indexed + estimate
            let indexed = stats.last_indexed_block;
            let chain_height = state.db.get_chain_height().await.unwrap_or(indexed);
            let blocks_behind = chain_height.saturating_sub(indexed);
            let sync_percentage = if chain_height > 0 {
                (indexed as f64 / chain_height as f64) * 100.0
            } else {
                0.0
            };

            let response = StatusResponse {
                version: env!("CARGO_PKG_VERSION").to_string(),
                uptime_seconds: get_uptime(),
                sync: SyncStatus {
                    indexed_blocks: indexed,
                    chain_height,
                    sync_percentage,
                    is_synced: blocks_behind <= 2,
                    blocks_behind,
                },
                stats,
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::<StatusResponse>::error(e.to_string())),
    }
}

// ==================== Inscriptions ====================

#[derive(Debug, Deserialize)]
pub struct PaginationParams {
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_page() -> u32 { 1 }
fn default_limit() -> u32 { 20 }

#[derive(Debug, Deserialize)]
pub struct InscriptionQueryParams {
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_limit")]
    limit: u32,
    /// Filter by content type (e.g., "image/png", "text/plain", "application/json")
    #[serde(rename = "type")]
    content_type: Option<String>,
    /// Include full content in response (default: false for list, true for single)
    #[serde(default)]
    include_content: Option<bool>,
    /// Sort order: "desc" (newest first) or "asc" (oldest first)
    #[serde(default = "default_sort")]
    sort: String,
}

fn default_sort() -> String { "desc".to_string() }

/// Inscription summary (without full content for list views)
#[derive(Debug, Serialize)]
pub struct InscriptionSummary {
    pub id: i64,
    pub inscription_id: String,
    pub inscription_number: u64,
    pub txid: String,
    pub vout: u32,
    pub content_type: String,
    pub content_size: u32,
    pub owner_address: Option<String>,
    pub current_location: Option<String>,
    pub block_height: u64,
    pub block_time: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

async fn get_inscriptions(
    State(state): State<Arc<AppState>>,
    Query(params): Query<InscriptionQueryParams>,
) -> impl IntoResponse {
    let limit = params.limit.min(100);
    let page = params.page.max(1);
    let offset = (page - 1) * limit;
    let include_content = params.include_content.unwrap_or(false);
    let sort_desc = params.sort.to_lowercase() != "asc";

    // Get total count (with optional filter)
    let total = match &params.content_type {
        Some(ct) => state.db.count_inscriptions_by_type(ct).await.unwrap_or(0),
        None => state.db.count_inscriptions().await.unwrap_or(0),
    };

    // Get inscriptions
    let result = match &params.content_type {
        Some(ct) => state.db.get_inscriptions_by_type(ct, limit, offset, sort_desc).await,
        None => state.db.get_inscriptions_paginated(limit, offset, sort_desc).await,
    };

    match result {
        Ok(inscriptions) => {
            // Convert to summary format
            let items: Vec<InscriptionSummary> = inscriptions
                .into_iter()
                .enumerate()
                .map(|(idx, insc)| InscriptionSummary {
                    id: insc.id.unwrap_or(0),
                    inscription_id: insc.inscription_id,
                    inscription_number: if sort_desc {
                        total.saturating_sub((offset + idx as u32) as u64)
                    } else {
                        (offset + idx as u32) as u64 + 1
                    },
                    txid: insc.txid,
                    vout: insc.vout,
                    content_type: insc.content_type,
                    content_size: insc.content_size,
                    owner_address: insc.owner_address,
                    current_location: insc.current_location,
                    block_height: insc.block_height,
                    block_time: insc.block_time,
                    content: if include_content { Some(insc.content) } else { None },
                })
                .collect();

            let response = PaginatedResponse {
                items,
                pagination: Pagination::new(page, limit, total),
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::<PaginatedResponse<InscriptionSummary>>::error(e.to_string())),
    }
}

async fn get_inscription(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.db.get_inscription(&id).await {
        Ok(Some(inscription)) => Json(ApiResponse::success(inscription)),
        Ok(None) => Json(ApiResponse::<Inscription>::error("Inscription not found")),
        Err(e) => Json(ApiResponse::<Inscription>::error(e.to_string())),
    }
}

async fn get_inscriptions_by_address(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
    Query(params): Query<PaginationParams>,
) -> impl IntoResponse {
    let limit = params.limit.min(100);
    let offset = (params.page - 1) * limit;
    let total = state.db.count_inscriptions_by_owner(&address).await.unwrap_or(0);
    
    match state.db.get_inscriptions_by_owner(&address, limit, offset).await {
        Ok(inscriptions) => {
            let response = PaginatedResponse {
                items: inscriptions,
                pagination: Pagination::new(params.page, limit, total),
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::<PaginatedResponse<Inscription>>::error(e.to_string())),
    }
}

// ==================== DAR-20 Tokens ====================

#[derive(Debug, Deserialize)]
pub struct TokenQueryParams {
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_limit")]
    limit: u32,
    /// Sort by: "deploy" (deploy time), "minted" (total minted), "holders" (holder count)
    #[serde(default = "default_token_sort")]
    sort: String,
}

fn default_token_sort() -> String { "deploy".to_string() }

/// Token summary with stats
#[derive(Debug, Serialize)]
pub struct TokenSummary {
    pub tick: String,
    pub max_supply: String,
    pub mint_limit: String,
    pub decimals: u8,
    pub total_minted: String,
    pub deploy_txid: String,
    pub deploy_address: String,
    pub block_height: u64,
    pub holders_count: u64,
    pub mints_count: u64,
    pub progress_percent: f64,
}

async fn get_tokens(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TokenQueryParams>,
) -> impl IntoResponse {
    let limit = params.limit.min(100);
    let page = params.page.max(1);
    let offset = (page - 1) * limit;

    let total = state.db.count_tokens().await.unwrap_or(0);

    match state.db.get_tokens_paginated(limit, offset, &params.sort).await {
        Ok(tokens) => {
            // Enrich with stats
            let mut items: Vec<TokenSummary> = Vec::new();
            for token in tokens {
                let holders_count = state.db.count_token_holders(&token.tick).await.unwrap_or(0);
                let mints_count = state.db.count_token_mints(&token.tick).await.unwrap_or(0);
                let total_minted: f64 = token.total_minted.parse().unwrap_or(0.0);
                let max_supply: f64 = token.max_supply.parse().unwrap_or(1.0);
                let progress_percent = (total_minted / max_supply) * 100.0;

                items.push(TokenSummary {
                    tick: token.tick,
                    max_supply: token.max_supply,
                    mint_limit: token.mint_limit,
                    decimals: token.decimals,
                    total_minted: token.total_minted,
                    deploy_txid: token.deploy_txid,
                    deploy_address: token.deploy_address,
                    block_height: token.block_height,
                    holders_count,
                    mints_count,
                    progress_percent,
                });
            }

            let response = PaginatedResponse {
                items,
                pagination: Pagination::new(page, limit, total),
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::<PaginatedResponse<TokenSummary>>::error(e.to_string())),
    }
}

#[derive(Debug, Serialize)]
pub struct TokenDetails {
    pub tick: String,
    pub max_supply: String,
    pub mint_limit: String,
    pub decimals: u8,
    pub total_minted: String,
    pub remaining_supply: String,
    pub deploy_txid: String,
    pub deploy_address: String,
    pub block_height: u64,
    pub block_time: u64,
    pub holders_count: u64,
    pub mints_count: u64,
    pub progress_percent: f64,
    pub is_fully_minted: bool,
}

async fn get_token(
    State(state): State<Arc<AppState>>,
    Path(tick): Path<String>,
) -> impl IntoResponse {
    match state.db.get_token(&tick).await {
        Ok(Some(token)) => {
            let holders_count = state.db.count_token_holders(&tick).await.unwrap_or(0);
            let mints_count = state.db.count_token_mints(&tick).await.unwrap_or(0);
            
            let total_minted: f64 = token.total_minted.parse().unwrap_or(0.0);
            let max_supply: f64 = token.max_supply.parse().unwrap_or(1.0);
            let progress_percent = (total_minted / max_supply) * 100.0;
            let remaining = max_supply - total_minted;

            let details = TokenDetails {
                tick: token.tick,
                max_supply: token.max_supply,
                mint_limit: token.mint_limit,
                decimals: token.decimals,
                total_minted: token.total_minted,
                remaining_supply: format!("{}", remaining as u64),
                deploy_txid: token.deploy_txid,
                deploy_address: token.deploy_address,
                block_height: token.block_height,
                block_time: token.block_time,
                holders_count,
                mints_count,
                progress_percent,
                is_fully_minted: remaining <= 0.0,
            };
            Json(ApiResponse::success(details))
        }
        Ok(None) => Json(ApiResponse::<TokenDetails>::error("Token not found")),
        Err(e) => Json(ApiResponse::<TokenDetails>::error(e.to_string())),
    }
}

#[derive(Debug, Deserialize)]
pub struct HoldersParams {
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_holders_limit")]
    limit: u32,
}

fn default_holders_limit() -> u32 { 100 }

/// Holder with percentage of supply
#[derive(Debug, Serialize)]
pub struct HolderInfo {
    pub rank: u32,
    pub address: String,
    /// Total balance (available + locked)
    pub balance: String,
    pub percentage: f64,
    /// Available balance (total - locked), if known
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_balance: Option<String>,
    /// Locked balance (e.g. in pending transfers), if known
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked_balance: Option<String>,
    /// Explicit total balance, if returned from DB (mirrors `balance`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_balance: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HoldersResponse {
    pub tick: String,
    pub total_holders: u64,
    pub holders: Vec<HolderInfo>,
    pub pagination: Pagination,
}

async fn get_token_holders(
    State(state): State<Arc<AppState>>,
    Path(tick): Path<String>,
    Query(params): Query<HoldersParams>,
) -> impl IntoResponse {
    let limit = params.limit.min(500);
    let page = params.page.max(1);
    let offset = (page - 1) * limit;

    // Get token for total supply calculation
    let token = match state.db.get_token(&tick).await {
        Ok(Some(t)) => t,
        Ok(None) => return Json(ApiResponse::<HoldersResponse>::error("Token not found")),
        Err(e) => return Json(ApiResponse::<HoldersResponse>::error(e.to_string())),
    };

    let total_minted: f64 = token.total_minted.parse().unwrap_or(1.0);
    let total_holders = state.db.count_token_holders(&tick).await.unwrap_or(0);

    match state.db.get_token_holders_paginated(&tick, limit, offset).await {
        Ok(balances) => {
            let holders: Vec<HolderInfo> = balances
                .into_iter()
                .enumerate()
                .map(|(idx, b)| {
                    // Prefer explicit total_balance from DB, fall back to balance field (which
                    // stores available balance in Dar20Balance)
                    let total_str = b
                        .total_balance
                        .clone()
                        .unwrap_or_else(|| b.balance.clone());
                    let total: f64 = total_str.parse().unwrap_or(0.0);

                    let percentage = if total_minted > 0.0 {
                        (total / total_minted) * 100.0
                    } else {
                        0.0
                    };

                    // Clone available balance string so we can reuse it in multiple fields
                    let available_str = b.balance.clone();

                    HolderInfo {
                        rank: offset + idx as u32 + 1,
                        address: b.address,
                        // Expose TOTAL balance via `balance` for holders table
                        balance: total_str.clone(),
                        percentage,
                        available_balance: Some(available_str),
                        locked_balance: b.locked_balance,
                        total_balance: Some(total_str),
                    }
                })
                .collect();

            let response = HoldersResponse {
                tick: tick.to_uppercase(),
                total_holders,
                holders,
                pagination: Pagination::new(page, limit, total_holders),
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::<HoldersResponse>::error(e.to_string())),
    }
}

#[derive(Debug, Deserialize)]
pub struct MintsParams {
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_limit")]
    limit: u32,
    /// Filter by address
    address: Option<String>,
    /// Filter by validity (true/false)
    valid: Option<bool>,
}

async fn get_token_mints(
    State(state): State<Arc<AppState>>,
    Path(tick): Path<String>,
    Query(params): Query<MintsParams>,
) -> impl IntoResponse {
    let limit = params.limit.min(100);
    let page = params.page.max(1);
    let offset = (page - 1) * limit;

    let total = state.db.count_token_mints_filtered(&tick, params.address.as_deref(), params.valid).await.unwrap_or(0);

    match state.db.get_mints_paginated(&tick, limit, offset, params.address.as_deref(), params.valid).await {
        Ok(mints) => {
            let response = PaginatedResponse {
                items: mints,
                pagination: Pagination::new(page, limit, total),
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::<PaginatedResponse<crate::models::Dar20Mint>>::error(e.to_string())),
    }
}

#[derive(Debug, Deserialize)]
pub struct TransfersParams {
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_limit")]
    limit: u32,
    /// Filter by sender address
    from: Option<String>,
    /// Filter by recipient address
    to: Option<String>,
    /// Filter by validity (true/false)
    valid: Option<bool>,
}

async fn get_token_transfers(
    State(state): State<Arc<AppState>>,
    Path(tick): Path<String>,
    Query(params): Query<TransfersParams>,
) -> impl IntoResponse {
    let limit = params.limit.min(100);
    let page = params.page.max(1);
    let offset = (page - 1) * limit;

    let total = state.db.count_token_transfers_filtered(
        &tick,
        params.from.as_deref(),
        params.to.as_deref(),
        params.valid,
    ).await.unwrap_or(0) as u64;

    match state.db.get_transfers_paginated(
        &tick,
        limit,
        offset,
        params.from.as_deref(),
        params.to.as_deref(),
        params.valid,
    ).await {
        Ok(transfers) => {
            let response = PaginatedResponse {
                items: transfers,
                pagination: Pagination::new(page, limit, total),
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::<PaginatedResponse<crate::models::Dar20Transfer>>::error(e.to_string())),
    }
}

// ==================== DAR-20 Balances ====================

/// Balance with token context
#[derive(Debug, Serialize)]
pub struct BalanceWithContext {
    pub tick: String,
    /// Total balance (available + locked)
    /// NOTE: This field now returns the **total** balance for compatibility
    /// with existing marketplace expectations.
    pub balance: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_balance: Option<String>, // Explicit total balance (same as `balance` when present)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked_balance: Option<String>, // Locked balance (in pending transfers)
    pub max_supply: String,
    pub total_minted: String,
    pub percentage_of_supply: f64,
}

async fn get_balances_by_address(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
) -> impl IntoResponse {
    match state.db.get_balances_by_address(&address).await {
        Ok(balances) => {
            let mut items: Vec<BalanceWithContext> = Vec::new();
            
            for balance in balances {
                if let Ok(Some(token)) = state.db.get_token(&balance.tick).await {
                    // Use TOTAL balance (available + locked) when calculating percentage
                    let total_str = balance
                        .total_balance
                        .clone()
                        .unwrap_or_else(|| balance.balance.clone());

                    let bal: f64 = total_str.parse().unwrap_or(0.0);
                    let total_minted: f64 = token.total_minted.parse().unwrap_or(1.0);
                    let percentage = if total_minted > 0.0 {
                        (bal / total_minted) * 100.0
                    } else {
                        0.0
                    };
                    
                    items.push(BalanceWithContext {
                        tick: balance.tick,
                        // Expose TOTAL balance via `balance` for callers
                        balance: total_str.clone(),
                        total_balance: Some(total_str),
                        locked_balance: balance.locked_balance.clone(),
                        max_supply: token.max_supply,
                        total_minted: token.total_minted,
                        percentage_of_supply: percentage,
                    });
                }
            }
            
            Json(ApiResponse::success(items))
        }
        Err(e) => Json(ApiResponse::<Vec<BalanceWithContext>>::error(e.to_string())),
    }
}

#[derive(Debug, Serialize)]
pub struct BalanceResponse {
    pub tick: String,
    pub address: String,
    pub balance: String,              // Available balance (total - locked)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_balance: Option<String>, // Total balance (including locked)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked_balance: Option<String>, // Locked balance (in pending transfers)
    pub rank: Option<u64>,
    pub percentage_of_supply: f64,
    pub total_holders: u64,
}

async fn get_balance(
    State(state): State<Arc<AppState>>,
    Path((tick, address)): Path<(String, String)>,
) -> impl IntoResponse {
    let tick_upper = tick.to_uppercase();
    
    // Get available balance (total - locked)
    let balance = match state.db.get_balance(&tick_upper, &address).await {
        Ok(b) => b,
        Err(e) => return Json(ApiResponse::<BalanceResponse>::error(e.to_string())),
    };

    // Get total and locked balances
    let total_balance = state.db.get_total_balance(&tick_upper, &address).await.ok();
    let locked_balance = state.db.get_locked_balance(&tick_upper, &address).await.ok();
    
    // Only include locked_balance if > 0
    let locked_balance_opt = locked_balance.and_then(|lb| {
        if lb != "0" && !lb.is_empty() {
            Some(lb)
        } else {
            None
        }
    });

    // Get token info for percentage calculation
    let (percentage, total_holders) = match state.db.get_token(&tick_upper).await {
        Ok(Some(token)) => {
            let bal: f64 = balance.parse().unwrap_or(0.0);
            let total_minted: f64 = token.total_minted.parse().unwrap_or(1.0);
            let pct = if total_minted > 0.0 { (bal / total_minted) * 100.0 } else { 0.0 };
            let holders = state.db.count_token_holders(&tick_upper).await.unwrap_or(0);
            (pct, holders)
        }
        _ => (0.0, 0),
    };

    // Get rank if balance > 0
    let rank = if balance != "0" {
        state.db.get_holder_rank(&tick_upper, &address).await.ok()
    } else {
        None
    };

    let response = BalanceResponse {
        tick: tick_upper,
        address,
        balance, // Available balance
        total_balance,
        locked_balance: locked_balance_opt,
        rank,
        percentage_of_supply: percentage,
        total_holders,
    };
    Json(ApiResponse::success(response))
}

// ==================== DashMap (like Bitmap) ====================

#[derive(Debug, Deserialize)]
pub struct DashMapQueryParams {
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_limit")]
    limit: u32,
    /// Sort: "newest" (by claim time) or "height" (by block height)
    #[serde(default = "default_dashmap_sort")]
    sort: String,
}

fn default_dashmap_sort() -> String { "newest".to_string() }

async fn get_dashmap_claims(
    State(state): State<Arc<AppState>>,
    Query(params): Query<DashMapQueryParams>,
) -> impl IntoResponse {
    let limit = params.limit.min(100);
    let page = params.page.max(1);
    let offset = (page - 1) * limit;

    let total = state.db.count_dashmap_claims().await.unwrap_or(0);

    match state.db.get_dashmap_claims_paginated(limit, offset, &params.sort).await {
        Ok(claims) => {
            let response = PaginatedResponse {
                items: claims,
                pagination: Pagination::new(page, limit, total),
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::<PaginatedResponse<DashMapClaim>>::error(e.to_string())),
    }
}

#[derive(Debug, Serialize)]
pub struct BlockClaimResponse {
    pub claimed: bool,
    pub block_height: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claim: Option<DashMapClaim>,
}

async fn get_dashmap_block(
    State(state): State<Arc<AppState>>,
    Path(height): Path<u64>,
) -> impl IntoResponse {
    match state.db.get_dashmap_claim(height).await {
        Ok(Some(claim)) => {
            let response = BlockClaimResponse {
                claimed: true,
                block_height: height,
                claim: Some(claim),
            };
            Json(ApiResponse::success(response))
        }
        Ok(None) => {
            let response = BlockClaimResponse {
                claimed: false,
                block_height: height,
                claim: None,
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::<BlockClaimResponse>::error(e.to_string())),
    }
}

#[derive(Debug, Serialize)]
pub struct DashMapOwnerResponse {
    pub address: String,
    pub total_blocks: u64,
    pub blocks: Vec<DashMapClaim>,
    pub pagination: Pagination,
}

async fn get_dashmap_by_address(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
    Query(params): Query<PaginationParams>,
) -> impl IntoResponse {
    let limit = params.limit.min(500);
    let page = params.page.max(1);
    let offset = (page - 1) * limit;

    let total = state.db.count_dashmap_by_owner(&address).await.unwrap_or(0);

    match state.db.get_dashmap_by_owner_paginated(&address, limit, offset).await {
        Ok(claims) => {
            let response = DashMapOwnerResponse {
                address,
                total_blocks: total,
                blocks: claims,
                pagination: Pagination::new(page, limit, total),
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::<DashMapOwnerResponse>::error(e.to_string())),
    }
}

#[derive(Debug, Serialize)]
pub struct TopClaimer {
    pub address: String,
    pub blocks_claimed: u64,
}

#[derive(Debug, Serialize)]
pub struct DashMapStats {
    pub total_claimed: u64,
    pub total_blocks_available: u64,
    pub claim_percentage: f64,
    pub unique_claimers: u64,
    pub top_claimers: Vec<TopClaimer>,
}

async fn get_dashmap_stats(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.db.count_dashmap_claims().await {
        Ok(total_claimed) => {
            let last_block = state.db.get_last_indexed_block().await.unwrap_or(0);
            let claim_percentage = if last_block > 0 {
                (total_claimed as f64 / last_block as f64) * 100.0
            } else {
                0.0
            };

            let unique_claimers = state.db.count_unique_dashmap_claimers().await.unwrap_or(0);
            let top_claimers = state.db.get_top_dashmap_claimers(10).await.unwrap_or_default();

            let stats = DashMapStats {
                total_claimed,
                total_blocks_available: last_block,
                claim_percentage,
                unique_claimers,
                top_claimers: top_claimers
                    .into_iter()
                    .map(|(address, count)| TopClaimer {
                        address,
                        blocks_claimed: count,
                    })
                    .collect(),
            };
            Json(ApiResponse::success(stats))
        }
        Err(e) => Json(ApiResponse::<DashMapStats>::error(e.to_string())),
    }
}

// ==================== DashDomain (like bitdomain) ====================

#[derive(Debug, Deserialize)]
pub struct DomainQueryParams {
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_limit")]
    limit: u32,
    /// Sort: "newest" (by registration time) or "name" (alphabetical)
    #[serde(default = "default_domain_sort")]
    sort: String,
}

fn default_domain_sort() -> String { "newest".to_string() }

async fn get_domains(
    State(state): State<Arc<AppState>>,
    Query(params): Query<DomainQueryParams>,
) -> impl IntoResponse {
    let limit = params.limit.min(100);
    let page = params.page.max(1);
    let offset = (page - 1) * limit;

    let total = state.db.count_domains().await.unwrap_or(0);

    match state.db.get_domains_paginated(limit, offset, &params.sort).await {
        Ok(domains) => {
            let response = PaginatedResponse {
                items: domains,
                pagination: Pagination::new(page, limit, total),
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::<PaginatedResponse<DashDomain>>::error(e.to_string())),
    }
}

#[derive(Debug, Serialize)]
pub struct DomainLookupResponse {
    pub name: String,
    pub full_name: String,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<DashDomain>,
}

async fn get_domain(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let name_clean = name.to_lowercase().trim_end_matches(".dash").to_string();
    let full_name = format!("{}.dash", name_clean);

    match state.db.get_domain(&name_clean).await {
        Ok(Some(domain)) => {
            let response = DomainLookupResponse {
                name: name_clean,
                full_name,
                available: false,
                domain: Some(domain),
            };
            Json(ApiResponse::success(response))
        }
        Ok(None) => {
            let response = DomainLookupResponse {
                name: name_clean,
                full_name,
                available: true,
                domain: None,
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::<DomainLookupResponse>::error(e.to_string())),
    }
}

#[derive(Debug, Serialize)]
pub struct DomainOwnerResponse {
    pub address: String,
    pub total_domains: u64,
    pub domains: Vec<DashDomain>,
    pub pagination: Pagination,
}

async fn get_domains_by_address(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
    Query(params): Query<PaginationParams>,
) -> impl IntoResponse {
    let limit = params.limit.min(500);
    let page = params.page.max(1);
    let offset = (page - 1) * limit;

    let total = state.db.count_domains_by_owner(&address).await.unwrap_or(0);

    match state.db.get_domains_by_owner_paginated(&address, limit, offset).await {
        Ok(domains) => {
            let response = DomainOwnerResponse {
                address,
                total_domains: total,
                domains,
                pagination: Pagination::new(page, limit, total),
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::<DomainOwnerResponse>::error(e.to_string())),
    }
}

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    #[serde(default)]
    q: String,
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_limit")]
    limit: u32,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub items: Vec<DashDomain>,
    pub pagination: Pagination,
}

async fn search_domains(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> impl IntoResponse {
    if params.q.is_empty() {
        return Json(ApiResponse::<SearchResponse>::error("Search query 'q' is required"));
    }

    let limit = params.limit.min(100);
    let page = params.page.max(1);
    let offset = (page - 1) * limit;

    let total = state.db.count_domains_search(&params.q).await.unwrap_or(0);

    match state.db.search_domains_paginated(&params.q, limit, offset).await {
        Ok(domains) => {
            let response = SearchResponse {
                query: params.q,
                items: domains,
                pagination: Pagination::new(page, limit, total),
            };
            Json(ApiResponse::success(response))
        }
        Err(e) => Json(ApiResponse::<SearchResponse>::error(e.to_string())),
    }
}

#[derive(Debug, Serialize)]
pub struct TopDomainOwner {
    pub address: String,
    pub domains_count: u64,
}

#[derive(Debug, Serialize)]
pub struct DomainStats {
    pub total_registered: u64,
    pub unique_owners: u64,
    pub top_owners: Vec<TopDomainOwner>,
}

async fn get_domain_stats(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.db.count_domains().await {
        Ok(total_registered) => {
            let unique_owners = state.db.count_unique_domain_owners().await.unwrap_or(0);
            let top_owners = state.db.get_top_domain_owners(10).await.unwrap_or_default();

            let stats = DomainStats {
                total_registered,
                unique_owners,
                top_owners: top_owners
                    .into_iter()
                    .map(|(address, count)| TopDomainOwner {
                        address,
                        domains_count: count,
                    })
                    .collect(),
            };
            Json(ApiResponse::success(stats))
        }
        Err(e) => Json(ApiResponse::<DomainStats>::error(e.to_string())),
    }
}

