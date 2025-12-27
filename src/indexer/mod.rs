use anyhow::Result;
use std::time::Duration;
use tracing::{debug, error, info, warn};

use crate::db::Database;
use crate::models::{Dar20Mint, Dar20Token, Dar20Transfer, Dar20Operation, DashDomain, DashMapClaim};
use crate::parser::{format_amount, get_first_output_address, parse_amount, parse_dar20, parse_dash_domain, parse_dashmap, parse_inscription, validate_ticker};
use crate::rpc::DashRpc;

/// Blockchain indexer for Dash inscriptions and DAR-20 tokens
pub struct Indexer {
    db: Database,
    rpc: DashRpc,
}

impl Indexer {
    /// Create new indexer
    pub fn new(db: Database, rpc: DashRpc) -> Self {
        Self { db, rpc }
    }

    /// Start indexing from a specific block
    pub async fn start(&self, start_block: u64) -> Result<()> {
        info!("Starting indexer from block {}", start_block);

        let mut current_block = start_block;

        loop {
            // Get current chain height
            let chain_height = match self.rpc.get_block_count().await {
                Ok(height) => {
                    // Store chain height in DB for status endpoint
                    let _ = self.db.set_chain_height(height).await;
                    height
                }
                Err(e) => {
                    error!("Failed to get block count: {}", e);
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    continue;
                }
            };

            // Index all available blocks
            while current_block <= chain_height {
                match self.index_block(current_block).await {
                    Ok(inscriptions_found) => {
                        if inscriptions_found > 0 {
                            info!(
                                "Block {}: {} inscriptions found",
                                current_block, inscriptions_found
                            );
                        } else {
                            debug!("Block {}: indexed", current_block);
                        }

                        // Update last indexed block
                        if let Err(e) = self.db.set_last_indexed_block(current_block).await {
                            error!("Failed to update last indexed block: {}", e);
                        }

                        current_block += 1;
                    }
                    Err(e) => {
                        error!("Failed to index block {}: {}", current_block, e);
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }

                // Log progress every 1000 blocks
                if current_block % 1000 == 0 {
                    info!(
                        "Progress: block {} / {} ({:.2}%)",
                        current_block,
                        chain_height,
                        (current_block as f64 / chain_height as f64) * 100.0
                    );
                }
            }

            // Wait for new blocks
            debug!("Caught up to block {}. Waiting for new blocks...", chain_height);
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    }

    /// Index a single block
    async fn index_block(&self, height: u64) -> Result<u32> {
        let block = self.rpc.get_block_by_height(height).await?;
        let mut inscriptions_found = 0;

        // Process each transaction in the block
        for txid in &block.tx {
            match self.process_transaction(txid, height, block.time).await {
                Ok(found) => {
                    if found {
                        inscriptions_found += 1;
                    }
                }
                Err(e) => {
                    // Log but don't fail the whole block
                    debug!("Error processing tx {}: {}", txid, e);
                }
            }
        }

        Ok(inscriptions_found)
    }

    /// Process a single transaction
    async fn process_transaction(&self, txid: &str, block_height: u64, block_time: u64) -> Result<bool> {
        let tx = self.rpc.get_raw_transaction(txid).await?;

        // First, check if any input spends an inscription UTXO (transfer detection)
        self.detect_inscription_transfers(&tx, block_height, block_time).await?;

        // Try to parse inscription
        let inscription = match parse_inscription(&tx) {
            Some(mut insc) => {
                insc.block_height = block_height;
                insc.block_time = block_time;
                insc
            }
            None => return Ok(false),
        };

        debug!(
            "Found inscription: {} (type: {}, size: {} bytes)",
            inscription.inscription_id, inscription.content_type, inscription.content_size
        );

        // Save inscription
        if let Err(e) = self.db.insert_inscription(&inscription).await {
            // Might be duplicate, ignore
            debug!("Failed to insert inscription: {}", e);
        }

        // Check for DAR-20
        if let Some(dar20) = parse_dar20(&inscription) {
            self.process_dar20(&dar20, &inscription, block_height, block_time)
                .await?;
        }

        // Check for DashMap
        if let Some(dashmap) = parse_dashmap(&inscription) {
            self.process_dashmap(&dashmap, &inscription, block_height, block_time)
                .await?;
        }

        // Check for DashDomain
        if let Some(domain) = parse_dash_domain(&inscription) {
            self.process_domain(&domain, &inscription, block_height, block_time)
                .await?;
        }

        Ok(true)
    }

    /// Detect and process inscription transfers (UTXO spending)
    async fn detect_inscription_transfers(&self, tx: &crate::models::RawTransaction, block_height: u64, block_time: u64) -> Result<()> {
        // Check each input to see if it spends an inscription UTXO
        for input in &tx.vin {
            if let (Some(spent_txid), Some(spent_vout)) = (&input.txid, input.vout) {
                let spent_location = format!("{}:{}", spent_txid, spent_vout);

                debug!(
                    "Checking if UTXO {} contains an inscription (tx: {})",
                    spent_location, tx.txid
                );

                // Get the spent UTXO value to find which output receives the UTXO
                let spent_utxo_value = match self.rpc.get_raw_transaction(spent_txid).await {
                    Ok(prev_tx) => {
                        if let Some(output) = prev_tx.vout.get(spent_vout as usize) {
                            Some(output.value)
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        warn!("Failed to get previous transaction {}: {}", spent_txid, e);
                        None
                    }
                };

                // Find which output receives the UTXO
                // For buy transactions: Output 0 = seller price, Output 1 = inscription, Output 2 = change
                // For regular sends: Output 0 = inscription, Output 1 = change (if any)
                let (new_vout, new_owner) = if let Some(spent_value) = spent_utxo_value {
                    // Try to find output with matching value (UTXO preserved)
                    let mut found_output: Option<(u32, String)> = None;
                    for (idx, output) in tx.vout.iter().enumerate() {
                        // Check if value matches (with small tolerance for floating point)
                        if (output.value - spent_value).abs() < 0.00000001 {
                            let addr = output.script_pub_key.address.clone()
                                .or_else(|| output.script_pub_key.addresses.as_ref()?.first().cloned())
                                .unwrap_or_else(|| "unknown".to_string());
                            found_output = Some((idx as u32, addr));
                            break;
                        }
                    }
                    found_output.unwrap_or_else(|| {
                        // Fallback: use first output if no match found
                        (0, get_first_output_address(tx).unwrap_or_else(|| "unknown".to_string()))
                    })
                } else {
                    // Fallback: use first output if we can't get spent UTXO value
                    (0, get_first_output_address(tx).unwrap_or_else(|| "unknown".to_string()))
                };

                let new_location = format!("{}:{}", tx.txid, new_vout);

                // Check if this UTXO contains an inscription
                // We need to check for DAR-20 transfers BEFORE updating the location,
                // because we need the original owner_address to identify the sender
                if let Ok(Some(inscription)) = self.db.get_inscription_by_location(&spent_location).await {
                    // Check if this is a DAR-20 transfer inscription BEFORE updating location
                    let is_dar20_transfer = if let Some(dar20) = parse_dar20(&inscription) {
                        dar20.operation == Dar20Operation::Transfer
                    } else {
                        false
                    };

                    if is_dar20_transfer {
                        // Get the transfer record to find the original sender
                        let sender = if let Ok(Some(transfer_record)) = self.db.get_transfer_by_txid(&inscription.txid).await {
                            // Use the from_address from the transfer record (original sender)
                            transfer_record.from_address
                        } else {
                            // Fallback to inscription owner_address if transfer record not found
                            inscription.owner_address.clone().unwrap_or_else(|| "unknown".to_string())
                        };
                        
                        // Execute the DAR-20 transfer
                        if let Err(e) = self.execute_dar20_transfer(
                            &parse_dar20(&inscription).unwrap(),
                            &inscription,
                            &sender,
                            &new_owner, // Recipient from spending tx output
                            block_height,
                            block_time,
                        ).await {
                            warn!(
                                "Failed to execute DAR-20 transfer for inscription {}: {}",
                                inscription.inscription_id, e
                            );
                        }
                    }

                    // Update inscription location (for all inscriptions, including DAR-20 transfers)
                    if let Err(e) = self.db.update_inscription_location(
                        &inscription.inscription_id,
                        &new_location,
                        &new_owner,
                    ).await {
                        warn!(
                            "Failed to update inscription {} location: {}",
                            inscription.inscription_id, e
                        );
                    } else {
                        info!(
                            "Inscription {} transferred from {} to {} (owner: {})",
                            inscription.inscription_id,
                            spent_location,
                            new_location,
                            new_owner
                        );
                    }
                }

                // Check if this UTXO contains a DashMap claim
                if let Ok(Some(claim)) = self.db.get_dashmap_by_location(&spent_location).await {
                    // DashMap claim is being transferred
                    if let Err(e) = self.db.update_dashmap_location(
                        claim.block_height,
                        &new_location,
                        &new_owner,
                    ).await {
                        warn!(
                            "Failed to update DashMap block {} location: {}",
                            claim.block_height, e
                        );
                    } else {
                        info!(
                            "DashMap block {} transferred from {} to {} (owner: {})",
                            claim.block_height,
                            spent_location,
                            new_location,
                            new_owner
                        );
                    }
                }

                // Check if this UTXO contains a DashDomain
                if let Ok(Some(domain)) = self.db.get_domain_by_location(&spent_location).await {
                    // DashDomain is being transferred
                    if let Err(e) = self.db.update_domain_location(
                        &domain.name,
                        &new_location,
                        &new_owner,
                    ).await {
                        warn!(
                            "Failed to update DashDomain {}.dash location: {}",
                            domain.name, e
                        );
                    } else {
                        info!(
                            "DashDomain {}.dash transferred from {} to {} (owner: {})",
                            domain.name,
                            spent_location,
                            new_location,
                            new_owner
                        );
                    }
                }

            }
        }

        Ok(())
    }

    /// Process DashDomain registration
    async fn process_domain(
        &self,
        domain_inscription: &crate::models::DashDomainInscription,
        inscription: &crate::models::Inscription,
        block_height: u64,
        block_time: u64,
    ) -> Result<()> {
        let name = &domain_inscription.name;

        // Check if domain already registered
        if self.db.is_domain_registered(name).await? {
            debug!(
                "Domain {}.dash already registered, ignoring duplicate",
                name
            );
            return Ok(());
        }

        let owner_address = inscription
            .owner_address
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        // Set current_location to inscription's current location (creation location initially)
        let current_location = inscription.current_location.clone();

        let domain = DashDomain {
            id: None,
            name: name.clone(),
            full_name: domain_inscription.full_name.clone(),
            owner_address: owner_address.clone(),
            inscription_id: inscription.inscription_id.clone(),
            current_location,
            registration_txid: inscription.txid.clone(),
            registration_block: block_height,
            registration_time: block_time,
            created_at: None,
        };

        self.db.insert_domain(&domain).await?;
        info!(
            "DashDomain: {}.dash registered by {} (inscription: {})",
            name, owner_address, inscription.inscription_id
        );

        Ok(())
    }

    /// Process DashMap claim
    async fn process_dashmap(
        &self,
        dashmap: &crate::models::DashMapInscription,
        inscription: &crate::models::Inscription,
        block_height: u64,
        block_time: u64,
    ) -> Result<()> {
        let claimed_block = dashmap.block_height;

        // Validate: claimed block must exist (be less than current chain height)
        // We allow claiming any block up to the current indexed block
        if claimed_block > block_height {
            warn!(
                "DashMap claim for future block {} at block {}, ignoring",
                claimed_block, block_height
            );
            return Ok(());
        }

        // Check if block already claimed
        if self.db.is_block_claimed(claimed_block).await? {
            debug!(
                "DashMap block {} already claimed, ignoring duplicate",
                claimed_block
            );
            return Ok(());
        }

        let owner_address = inscription
            .owner_address
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        // Set current_location to inscription's current location (creation location initially)
        let current_location = inscription.current_location.clone();

        let claim = DashMapClaim {
            id: None,
            block_height: claimed_block,
            owner_address: owner_address.clone(),
            inscription_id: inscription.inscription_id.clone(),
            current_location,
            claim_txid: inscription.txid.clone(),
            claim_block_height: block_height,
            claim_time: block_time,
            created_at: None,
        };

        self.db.insert_dashmap_claim(&claim).await?;
        info!(
            "DashMap: Block {} claimed by {} (inscription: {})",
            claimed_block, owner_address, inscription.inscription_id
        );

        Ok(())
    }

    /// Process DAR-20 operation
    async fn process_dar20(
        &self,
        dar20: &crate::models::Dar20Inscription,
        inscription: &crate::models::Inscription,
        block_height: u64,
        block_time: u64,
    ) -> Result<()> {
        let tick = dar20.tick.to_uppercase();

        // Validate ticker
        if !validate_ticker(&tick) {
            warn!("Invalid DAR-20 ticker: {}", tick);
            return Ok(());
        }

        match dar20.operation {
            Dar20Operation::Deploy => {
                self.process_deploy(dar20, inscription, block_height, block_time)
                    .await?;
            }
            Dar20Operation::Mint => {
                self.process_mint(dar20, inscription, block_height, block_time)
                    .await?;
            }
            Dar20Operation::Transfer => {
                self.process_transfer(dar20, inscription, block_height, block_time)
                    .await?;
            }
        }

        Ok(())
    }

    /// Process DAR-20 deploy operation
    /// Canonical BRC-20 rules:
    /// - First valid deploy wins (all later deploys with same tick are ignored)
    /// - tick must be exactly 4 ASCII characters
    /// - max > 0, lim > 0, lim ≤ max
    /// - Parameters are immutable once deployed
    async fn process_deploy(
        &self,
        dar20: &crate::models::Dar20Inscription,
        inscription: &crate::models::Inscription,
        block_height: u64,
        block_time: u64,
    ) -> Result<()> {
        let tick = dar20.tick.to_uppercase();

        // Check if token already exists (first deploy wins)
        if let Some(_existing) = self.db.get_token(&tick).await? {
            warn!("DAR-20 token {} already deployed, ignoring duplicate deploy", tick);
            return Ok(());
        }

        // Get required fields
        let max_supply = match &dar20.max {
            Some(max) => max.clone(),
            None => {
                warn!("DAR-20 deploy missing max supply, ignoring");
                return Ok(());
            }
        };

        let mint_limit = dar20.lim.clone().unwrap_or_else(|| max_supply.clone());
        let decimals = dar20.dec.unwrap_or(18);

        // Canonical BRC-20 validation: max > 0, lim > 0, lim ≤ max
        let max_amount = parse_amount(&max_supply, decimals).unwrap_or(0);
        let lim_amount = parse_amount(&mint_limit, decimals).unwrap_or(0);

        if max_amount == 0 {
            warn!("DAR-20 deploy max supply must be > 0, ignoring");
            return Ok(());
        }

        if lim_amount == 0 {
            warn!("DAR-20 deploy mint limit must be > 0, ignoring");
            return Ok(());
        }

        if lim_amount > max_amount {
            warn!(
                "DAR-20 deploy mint limit {} exceeds max supply {}, ignoring",
                mint_limit, max_supply
            );
            return Ok(());
        }

        let deploy_address = inscription
            .owner_address
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        let token = Dar20Token {
            id: None,
            tick: tick.clone(),
            max_supply,
            mint_limit,
            decimals,
            total_minted: "0".to_string(),
            deploy_txid: inscription.txid.clone(),
            deploy_address,
            block_height,
            block_time,
            created_at: None,
        };

        self.db.insert_token(&token).await?;
        info!("DAR-20 token deployed: {} (max: {}, lim: {})", tick, token.max_supply, token.mint_limit);

        Ok(())
    }

    /// Process DAR-20 mint operation
    /// Canonical BRC-20 rules:
    /// - Token must be deployed
    /// - amt ≤ lim
    /// - total_minted + amt ≤ max
    /// - Invalid mints are completely ignored (not recorded)
    /// - Tokens go to address controlling the mint inscription UTXO
    /// - Permissionless, first-come-first-served
    /// - Stops permanently once max reached
    async fn process_mint(
        &self,
        dar20: &crate::models::Dar20Inscription,
        inscription: &crate::models::Inscription,
        block_height: u64,
        block_time: u64,
    ) -> Result<()> {
        let tick = dar20.tick.to_uppercase();

        // Get token (must be deployed)
        let token = match self.db.get_token(&tick).await? {
            Some(t) => t,
            None => {
                warn!("DAR-20 mint for non-existent token: {}, ignoring", tick);
                return Ok(());
            }
        };

        // Get mint amount
        let amount_str = match &dar20.amt {
            Some(amt) => amt.clone(),
            None => {
                warn!("DAR-20 mint missing amount, ignoring");
                return Ok(());
            }
        };

        // Parse amounts
        let mint_amount = match parse_amount(&amount_str, token.decimals) {
            Some(a) => a,
            None => {
                warn!("Invalid mint amount: {}, ignoring", amount_str);
                return Ok(());
            }
        };

        let mint_limit = parse_amount(&token.mint_limit, token.decimals).unwrap_or(u128::MAX);
        let max_supply = parse_amount(&token.max_supply, token.decimals).unwrap_or(u128::MAX);
        let total_minted = parse_amount(&token.total_minted, token.decimals).unwrap_or(0);

        // Canonical BRC-20 validation: amt ≤ lim
        if mint_amount > mint_limit {
            warn!(
                "Mint amount {} exceeds limit {} for {}, ignoring",
                amount_str, token.mint_limit, tick
            );
            return Ok(()); // Completely ignore invalid mint
        }

        // Canonical BRC-20 validation: total_minted + amt ≤ max
        // If exceeds max, completely ignore (no partial mints)
        if total_minted + mint_amount > max_supply {
            if total_minted >= max_supply {
                warn!("Token {} fully minted, ignoring mint", tick);
            } else {
                warn!(
                    "Mint amount {} would exceed max supply {} for {} (current: {}), ignoring",
                    amount_str, token.max_supply, tick, token.total_minted
                );
            }
            return Ok(()); // Completely ignore invalid mint
        }

        // Mint is valid - record and execute
        let to_address = inscription
            .owner_address
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        let mint = Dar20Mint {
            id: None,
            tick: tick.clone(),
            amount: amount_str.clone(),
            to_address: to_address.clone(),
            txid: inscription.txid.clone(),
            block_height,
            block_time,
            valid: true, // All recorded mints are valid (invalid ones are ignored)
            created_at: None,
        };

        self.db.insert_mint(&mint).await?;

        // Update total minted
        let new_total = total_minted + mint_amount;
        self.db
            .update_token_minted(&tick, &format_amount(new_total, token.decimals))
            .await?;

        // Update total balance (minting adds to total balance, not locked)
        // Tokens go to address controlling the mint inscription UTXO
        let current_total = self.db.get_total_balance(&tick, &to_address).await?;
        let current = parse_amount(&current_total, token.decimals).unwrap_or(0);
        let new_balance = current + mint_amount;
        self.db
            .update_balance(&tick, &to_address, &format_amount(new_balance, token.decimals))
            .await?;

        info!(
            "DAR-20 mint: {} {} to {} (total_minted: {}/{})",
            amount_str,
            tick,
            to_address,
            format_amount(new_total, token.decimals),
            token.max_supply
        );

        Ok(())
    }

    /// Process DAR-20 transfer operation
    async fn process_transfer(
        &self,
        dar20: &crate::models::Dar20Inscription,
        inscription: &crate::models::Inscription,
        block_height: u64,
        block_time: u64,
    ) -> Result<()> {
        let tick = dar20.tick.to_uppercase();

        // Get token
        let token = match self.db.get_token(&tick).await? {
            Some(t) => t,
            None => {
                warn!("DAR-20 transfer for non-existent token: {}", tick);
                return Ok(());
            }
        };

        // Get transfer amount
        let amount_str = match &dar20.amt {
            Some(amt) => amt.clone(),
            None => {
                warn!("DAR-20 transfer missing amount");
                return Ok(());
            }
        };

        let transfer_amount = match parse_amount(&amount_str, token.decimals) {
            Some(a) => a,
            None => {
                warn!("Invalid transfer amount: {}", amount_str);
                return Ok(());
            }
        };

        // Canonical BRC-20 transfer logic:
        // Step A: When transfer inscription is created, lock tokens
        // - Check available balance (total - locked)
        // - Lock tokens: increase locked_balance
        // - This prevents double-spending

        let from_address = inscription
            .owner_address
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        // Check available balance (total - locked)
        let sender_balance_str = self.db.get_balance(&tick, &from_address).await?;
        let sender_balance = parse_amount(&sender_balance_str, token.decimals).unwrap_or(0);

        let valid = sender_balance >= transfer_amount;

        let transfer = Dar20Transfer {
            id: None,
            tick: tick.clone(),
            amount: amount_str.clone(),
            from_address: from_address.clone(),
            to_address: "pending".to_string(), // Transfer inscribed but not yet executed
            txid: inscription.txid.clone(),
            block_height,
            block_time,
            valid,
            created_at: None,
        };

        self.db.insert_transfer(&transfer).await?;

        if valid {
            // Lock tokens: increase locked_balance (canonical BRC-20 behavior)
            // This prevents the same balance from being used in multiple transfers
            if let Err(e) = self.db.lock_balance(&tick, &from_address, &amount_str).await {
                warn!(
                    "Failed to lock balance for DAR-20 transfer: {} {} from {}: {}",
                    amount_str, tick, from_address, e
                );
            } else {
                info!(
                    "DAR-20 transfer inscription created: {} {} locked from {} (available: {})",
                    amount_str, tick, from_address, sender_balance_str
                );
            }
        } else {
            warn!(
                "Invalid DAR-20 transfer: {} has {} available but tried to transfer {} {}",
                from_address, sender_balance_str, amount_str, tick
            );
        }

        Ok(())
    }

    /// Execute DAR-20 transfer when transfer inscription UTXO is spent
    async fn execute_dar20_transfer(
        &self,
        dar20: &crate::models::Dar20Inscription,
        inscription: &crate::models::Inscription,
        sender: &str,
        recipient: &str,
        _block_height: u64,
        _block_time: u64,
    ) -> Result<()> {
        use anyhow::Context;
        
        let tick = dar20.tick.to_uppercase();
        let amount_str = dar20.amt.as_ref()
            .ok_or_else(|| anyhow::anyhow!("DAR-20 transfer missing amount"))?;
        
        // Get token
        let token = self.db.get_token(&tick).await?
            .ok_or_else(|| anyhow::anyhow!("Token {} not found", tick))?;
        
        // Parse amounts
        let transfer_amount = parse_amount(amount_str, token.decimals)
            .ok_or_else(|| anyhow::anyhow!("Invalid transfer amount: {}", amount_str))?;
        
        // Canonical BRC-20 transfer logic:
        // Step B: When transfer inscription UTXO is spent, unlock and transfer tokens
        // - Unlock from sender: decrease locked_balance
        // - Deduct from sender's total balance
        // - Add to recipient's total balance
        
        // Check that tokens are actually locked (should be, but verify)
        let sender_locked_str = self.db.get_locked_balance(&tick, sender).await?;
        let sender_locked = parse_amount(&sender_locked_str, token.decimals).unwrap_or(0);
        
        // Get sender's total balance to verify sufficient funds
        let sender_total_str = self.db.get_total_balance(&tick, sender).await?;
        let sender_total = parse_amount(&sender_total_str, token.decimals).unwrap_or(0);
        
        // Verify we have enough locked tokens (should match transfer amount)
        if sender_locked < transfer_amount {
            warn!(
                "Insufficient locked balance for DAR-20 transfer execution: {} has {} locked but needs {} {}",
                sender, sender_locked_str, amount_str, tick
            );
            // Update transfer record to mark as invalid
            if let Err(e) = self.db.update_transfer_recipient(
                &inscription.txid,
                recipient,
                false, // invalid
            ).await {
                warn!("Failed to update transfer record: {}", e);
            }
            return Ok(()); // Invalid transfer, but don't fail the whole transaction
        }
        
        // Verify sender has sufficient total balance
        if sender_total < transfer_amount {
            warn!(
                "Insufficient total balance for DAR-20 transfer execution: {} has {} total but needs {} {}",
                sender, sender_total_str, amount_str, tick
            );
            // Update transfer record to mark as invalid
            if let Err(e) = self.db.update_transfer_recipient(
                &inscription.txid,
                recipient,
                false, // invalid
            ).await {
                warn!("Failed to update transfer record: {}", e);
            }
            return Ok(()); // Invalid transfer, but don't fail the whole transaction
        }
        
        // Step 1: Unlock tokens from sender (decrease locked_balance)
        self.db.unlock_balance(&tick, sender, &amount_str).await
            .with_context(|| format!("Failed to unlock balance for {}", sender))?;
        
        // Step 2: Deduct from sender's total balance
        let new_sender_total = sender_total - transfer_amount;
        self.db.update_balance(
            &tick,
            sender,
            &format_amount(new_sender_total, token.decimals)
        ).await
            .with_context(|| format!("Failed to update sender balance for {}", sender))?;
        
        // Step 3: Add to recipient's total balance
        let recipient_total_str = self.db.get_total_balance(&tick, recipient).await?;
        let recipient_total = parse_amount(&recipient_total_str, token.decimals).unwrap_or(0);
        let new_recipient_total = recipient_total + transfer_amount;
        self.db.update_balance(
            &tick,
            recipient,
            &format_amount(new_recipient_total, token.decimals)
        ).await
            .with_context(|| format!("Failed to update recipient balance for {}", recipient))?;
        
        // Update transfer record with recipient and mark as executed
        self.db.update_transfer_recipient(
            &inscription.txid,
            recipient,
            true, // valid
        ).await
            .with_context(|| format!("Failed to update transfer record for {}", inscription.txid))?;
        
        info!(
            "DAR-20 transfer executed: {} {} from {} to {} (inscription: {})",
            amount_str, tick, sender, recipient, inscription.inscription_id
        );
        
        Ok(())
    }
}

