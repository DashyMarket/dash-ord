use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;
use tracing::{debug, error};

use crate::models::{Block, BlockchainInfo, RawTransaction};

/// Dash RPC client
#[derive(Clone)]
pub struct DashRpc {
    client: Client,
    url: String,
    user: String,
    password: String,
}

#[derive(Debug, Serialize)]
struct RpcRequest {
    jsonrpc: &'static str,
    id: &'static str,
    method: String,
    params: Vec<Value>,
}

#[derive(Debug, Deserialize)]
struct RpcResponse<T> {
    result: Option<T>,
    error: Option<RpcError>,
    #[allow(dead_code)]
    id: String,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i32,
    message: String,
}

impl DashRpc {
    /// Create new RPC client from environment variables
    pub fn from_env() -> Result<Self> {
        let url = env::var("DASH_RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:9998".to_string());
        let user = env::var("DASH_RPC_USER").unwrap_or_else(|_| "dashrpc".to_string());
        let password = env::var("DASH_RPC_PASSWORD")
            .map_err(|_| anyhow!("DASH_RPC_PASSWORD environment variable not set"))?;

        Ok(Self::new(url, user, password))
    }

    /// Create new RPC client with explicit parameters
    pub fn new(url: String, user: String, password: String) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            url,
            user,
            password,
        }
    }

    /// Make RPC call
    async fn call<T: for<'de> Deserialize<'de>>(&self, method: &str, params: Vec<Value>) -> Result<T> {
        let request = RpcRequest {
            jsonrpc: "1.0",
            id: "dash-indexer",
            method: method.to_string(),
            params,
        };

        debug!("RPC call: {} with {} params", method, request.params.len());

        let response = self
            .client
            .post(&self.url)
            .basic_auth(&self.user, Some(&self.password))
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            error!("RPC error: {} - {}", status, body);
            return Err(anyhow!("RPC request failed: {} - {}", status, body));
        }

        let rpc_response: RpcResponse<T> = serde_json::from_str(&body)?;

        if let Some(error) = rpc_response.error {
            return Err(anyhow!("RPC error {}: {}", error.code, error.message));
        }

        rpc_response
            .result
            .ok_or_else(|| anyhow!("RPC response missing result"))
    }

    /// Get blockchain info
    pub async fn get_blockchain_info(&self) -> Result<BlockchainInfo> {
        self.call("getblockchaininfo", vec![]).await
    }

    /// Get block hash by height
    pub async fn get_block_hash(&self, height: u64) -> Result<String> {
        self.call("getblockhash", vec![json!(height)]).await
    }

    /// Get block by hash
    pub async fn get_block(&self, hash: &str) -> Result<Block> {
        self.call("getblock", vec![json!(hash), json!(1)]).await
    }

    /// Get block by height
    pub async fn get_block_by_height(&self, height: u64) -> Result<Block> {
        let hash = self.get_block_hash(height).await?;
        self.get_block(&hash).await
    }

    /// Get raw transaction (decoded)
    pub async fn get_raw_transaction(&self, txid: &str) -> Result<RawTransaction> {
        self.call("getrawtransaction", vec![json!(txid), json!(true)])
            .await
    }

    /// Get raw transaction hex
    pub async fn get_raw_transaction_hex(&self, txid: &str) -> Result<String> {
        self.call("getrawtransaction", vec![json!(txid), json!(false)])
            .await
    }

    /// Get multiple transactions in batch
    pub async fn get_transactions_batch(&self, txids: &[String]) -> Result<Vec<RawTransaction>> {
        let mut transactions = Vec::with_capacity(txids.len());

        // Process in chunks to avoid overwhelming the node
        for chunk in txids.chunks(50) {
            let futures: Vec<_> = chunk
                .iter()
                .map(|txid| self.get_raw_transaction(txid))
                .collect();

            let results = futures::future::join_all(futures).await;

            for result in results {
                match result {
                    Ok(tx) => transactions.push(tx),
                    Err(e) => {
                        error!("Failed to fetch transaction: {}", e);
                        // Continue with other transactions
                    }
                }
            }
        }

        Ok(transactions)
    }

    /// Get best block hash
    pub async fn get_best_block_hash(&self) -> Result<String> {
        self.call("getbestblockhash", vec![]).await
    }

    /// Get block count
    pub async fn get_block_count(&self) -> Result<u64> {
        self.call("getblockcount", vec![]).await
    }

    /// Send raw transaction
    pub async fn send_raw_transaction(&self, hex: &str) -> Result<String> {
        self.call("sendrawtransaction", vec![json!(hex)]).await
    }

    /// Decode raw transaction
    pub async fn decode_raw_transaction(&self, hex: &str) -> Result<RawTransaction> {
        self.call("decoderawtransaction", vec![json!(hex)]).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires running Dash node
    async fn test_get_blockchain_info() {
        dotenvy::dotenv().ok();
        let rpc = DashRpc::from_env().unwrap();
        let info = rpc.get_blockchain_info().await.unwrap();
        assert!(!info.chain.is_empty());
        assert!(info.blocks > 0);
    }
}

