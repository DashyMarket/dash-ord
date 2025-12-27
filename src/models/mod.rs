use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Raw transaction structure from Dash RPC
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawTransaction {
    pub txid: String,
    pub version: i32,
    pub size: u32,
    #[serde(default)]
    pub locktime: u32,
    pub vin: Vec<TxInput>,
    pub vout: Vec<TxOutput>,
    #[serde(default)]
    pub blockhash: Option<String>,
    #[serde(default)]
    pub blockheight: Option<u64>,
    #[serde(default)]
    pub confirmations: Option<u32>,
    #[serde(default)]
    pub time: Option<u64>,
    #[serde(default)]
    pub blocktime: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxInput {
    #[serde(default)]
    pub txid: Option<String>,
    #[serde(default)]
    pub vout: Option<u32>,
    #[serde(rename = "scriptSig")]
    pub script_sig: Option<ScriptSig>,
    #[serde(default)]
    pub sequence: u64,
    /// For coinbase transactions
    #[serde(default)]
    pub coinbase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptSig {
    pub asm: String,
    pub hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxOutput {
    pub value: f64,
    pub n: u32,
    #[serde(rename = "scriptPubKey")]
    pub script_pub_key: ScriptPubKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptPubKey {
    pub asm: String,
    pub hex: String,
    #[serde(rename = "type")]
    pub script_type: Option<String>,
    #[serde(default)]
    pub addresses: Option<Vec<String>>,
    #[serde(default)]
    pub address: Option<String>,
}

/// Blockchain info from RPC
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockchainInfo {
    pub chain: String,
    pub blocks: u64,
    pub headers: u64,
    pub bestblockhash: String,
    pub difficulty: f64,
    #[serde(default)]
    pub mediantime: u64,
    pub verificationprogress: f64,
    #[serde(default)]
    pub chainwork: String,
    #[serde(default)]
    pub size_on_disk: u64,
    pub pruned: bool,
}

/// Block structure from RPC
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub hash: String,
    pub confirmations: i64,
    pub size: u32,
    pub height: u64,
    pub version: i32,
    #[serde(rename = "merkleroot")]
    pub merkle_root: String,
    pub tx: Vec<String>,
    pub time: u64,
    #[serde(rename = "mediantime")]
    pub median_time: u64,
    pub nonce: u64,
    pub bits: String,
    pub difficulty: f64,
    #[serde(rename = "previousblockhash")]
    pub previous_block_hash: Option<String>,
    #[serde(rename = "nextblockhash")]
    pub next_block_hash: Option<String>,
}

/// Parsed inscription data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inscription {
    pub id: Option<i64>,
    pub inscription_id: String,       // Immutable creation identifier (txid:vout format)
    pub txid: String,                  // Creation transaction ID
    pub vout: u32,                     // Creation output index
    pub content_type: String,
    pub content: String,              // Hex-encoded content
    pub content_size: u32,
    pub owner_address: Option<String>, // Current owner (updated on transfer)
    pub current_location: Option<String>, // Current UTXO location (txid:vout format, updated on transfer)
    pub block_height: u64,
    pub block_time: u64,
    pub created_at: Option<DateTime<Utc>>,
}

/// DAR-20 operation types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Dar20Operation {
    Deploy,
    Mint,
    Transfer,
}

/// DAR-20 inscription data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dar20Inscription {
    pub protocol: String,             // "dar-20"
    pub operation: Dar20Operation,
    pub tick: String,                 // Token ticker (e.g., "DASH")
    pub max: Option<String>,          // Max supply (deploy)
    pub lim: Option<String>,          // Mint limit per tx (deploy)
    pub amt: Option<String>,          // Amount (mint/transfer)
    pub dec: Option<u8>,              // Decimals (deploy, default 18)
}

/// DAR-20 token deployment record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dar20Token {
    pub id: Option<i64>,
    pub tick: String,
    pub max_supply: String,
    pub mint_limit: String,
    pub decimals: u8,
    pub total_minted: String,
    pub deploy_txid: String,
    pub deploy_address: String,
    pub block_height: u64,
    pub block_time: u64,
    pub created_at: Option<DateTime<Utc>>,
}

/// DAR-20 mint record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dar20Mint {
    pub id: Option<i64>,
    pub tick: String,
    pub amount: String,
    pub to_address: String,
    pub txid: String,
    pub block_height: u64,
    pub block_time: u64,
    pub valid: bool,                  // Whether mint was within limits
    pub created_at: Option<DateTime<Utc>>,
}

/// DAR-20 transfer record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dar20Transfer {
    pub id: Option<i64>,
    pub tick: String,
    pub amount: String,
    pub from_address: String,
    pub to_address: String,
    pub txid: String,
    pub block_height: u64,
    pub block_time: u64,
    pub valid: bool,                  // Whether sender had sufficient balance
    pub created_at: Option<DateTime<Utc>>,
}

/// DAR-20 balance for an address
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dar20Balance {
    pub id: Option<i64>,
    pub tick: String,
    pub address: String,
    pub balance: String,              // Available balance (total - locked)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_balance: Option<String>, // Total balance (including locked)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked_balance: Option<String>, // Locked balance (in pending transfers)
    pub updated_at: Option<DateTime<Utc>>,
}

/// DashMap claim record (like Bitmap)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashMapClaim {
    pub id: Option<i64>,
    pub block_height: u64,            // The claimed block
    pub owner_address: String,        // Current owner (updated on transfer)
    pub inscription_id: String,
    pub current_location: Option<String>, // Current UTXO location (txid:vout format, updated on transfer)
    pub claim_txid: String,
    pub claim_block_height: u64,      // Block where claim was made
    pub claim_time: u64,
    pub created_at: Option<DateTime<Utc>>,
}

/// DashDomain registration record (like bitdomain)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashDomain {
    pub id: Option<i64>,
    pub name: String,                 // Domain name without .dash (e.g., "alice")
    pub full_name: String,            // Full domain (e.g., "alice.dash")
    pub owner_address: String,        // Current owner (updated on transfer)
    pub inscription_id: String,
    pub current_location: Option<String>, // Current UTXO location (txid:vout format, updated on transfer)
    pub registration_txid: String,
    pub registration_block: u64,
    pub registration_time: u64,
    pub created_at: Option<DateTime<Utc>>,
}

/// Parsed DashDomain inscription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashDomainInscription {
    pub name: String,                 // Domain name without .dash
    pub full_name: String,            // Full domain with .dash
}

/// Parsed DashMap inscription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashMapInscription {
    pub block_height: u64,            // The block being claimed
}

/// Indexer statistics
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IndexerStats {
    pub total_inscriptions: u64,
    pub total_tokens: u64,
    pub total_holders: u64,
    pub total_mints: u64,
    pub total_transfers: u64,
    pub total_dashmap_claims: u64,
    pub total_domains: u64,
    pub last_indexed_block: u64,
}

/// API response wrapper
#[derive(Debug, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(msg.into()),
        }
    }
}

/// Pagination parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pagination {
    pub page: u32,
    pub limit: u32,
    pub total: u64,
    pub total_pages: u32,
}

impl Pagination {
    pub fn new(page: u32, limit: u32, total: u64) -> Self {
        let total_pages = ((total as f64) / (limit as f64)).ceil() as u32;
        Self {
            page,
            limit,
            total,
            total_pages,
        }
    }
}

/// Paginated response
#[derive(Debug, Serialize, Deserialize)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    pub pagination: Pagination,
}

