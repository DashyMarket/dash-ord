use tracing::debug;

use crate::models::{Dar20Inscription, Dar20Operation, DashDomainInscription, DashMapInscription, Inscription, RawTransaction};

/// Parse inscription from a transaction
/// Looks for "ord" protocol in scriptSig
pub fn parse_inscription(tx: &RawTransaction) -> Option<Inscription> {
    for input in &tx.vin {
        if let Some(script_sig) = &input.script_sig {
            if let Some(inscription) = parse_script_sig_inscription(&script_sig.hex) {
                let inscription_id = format!("{}:0", tx.txid);
                let current_location = inscription_id.clone(); // Initially, location is same as creation
                return Some(Inscription {
                    id: None,
                    inscription_id,
                    txid: tx.txid.clone(),
                    vout: 0,
                    content_type: inscription.0,
                    content: inscription.1.clone(),
                    content_size: (inscription.1.len() / 2) as u32,
                    owner_address: get_first_output_address(tx),
                    current_location: Some(current_location),
                    block_height: tx.blockheight.unwrap_or(0),
                    block_time: tx.blocktime.unwrap_or(0),
                    created_at: None,
                });
            }
        }
    }
    None
}

/// Parse scriptSig for inscription data
/// Format: "ord" <1> <content-type> <0> <content> <signature> <redeem_script>
fn parse_script_sig_inscription(script_hex: &str) -> Option<(String, String)> {
    let script = hex::decode(script_hex).ok()?;
    let chunks = parse_script_chunks(&script)?;

    if chunks.len() < 5 {
        return None;
    }

    // First chunk should be "ord"
    let protocol = String::from_utf8(chunks[0].clone()).ok()?;
    if protocol != "ord" {
        return None;
    }

    debug!("Found 'ord' protocol inscription");

    // chunks[1] = marker (1)
    // chunks[2] = content-type
    // chunks[3] = separator (0 or empty)
    // chunks[4+] = content (may span multiple chunks for large content)
    // Last chunks = signature + redeem_script

    let content_type = String::from_utf8(chunks[2].clone()).ok()?;
    
    // Concatenate content chunks
    // Content starts at chunks[4] and may span multiple chunks for large content
    // Format: "ord" <1> <content-type> <0> <content...> <signature?> <redeem_script>
    // Large content is split across multiple OP_PUSHDATA chunks (e.g., chunks 4, 6, 8)
    // Opcodes like OP_1, OP_0 appear between content chunks but should be skipped
    // We need to concatenate all DATA chunks from index 4 onwards until signature/redeem_script
    let mut content_bytes = Vec::new();
    
    // Start from chunk 4 (content starts here)
    // Concatenate all data chunks, skipping opcode chunks (OP_1, OP_0, etc.)
    // Stop when we detect signature (65-72 bytes starting with 0x30) or redeem_script
    if chunks.len() > 4 {
        for i in 4..chunks.len() {
            let chunk = &chunks[i];
            
            // Skip empty chunks (OP_0)
            if chunk.is_empty() {
                continue;
            }
            
            // Skip opcode chunks: single-byte chunks with values 1-16 (OP_1 to OP_16)
            // These are parsed as [1], [2], etc. by parse_script_chunks
            if chunk.len() == 1 && chunk[0] >= 1 && chunk[0] <= 16 {
                continue;
            }
            
            // Detect signature: typically 65-72 bytes starting with 0x30 (DER encoding)
            if chunk.len() >= 65 && chunk.len() <= 72 && chunk[0] == 0x30 {
                // This is likely a signature, stop here (don't include it)
                break;
            }
            
            // Detect redeem_script: typically shorter, at the end
            // If we've already collected substantial content and this is a small chunk, might be redeem_script
            if content_bytes.len() > 100 && chunk.len() < 50 && i == chunks.len() - 1 {
                // Likely redeem_script at the end, don't include it
                break;
            }
            
            // Include this chunk in content (it's actual data)
            content_bytes.extend_from_slice(chunk);
        }
    }
    
    // Fallback: if we got nothing, use chunks[4]
    if content_bytes.is_empty() && chunks.len() > 4 {
        content_bytes = chunks[4].clone();
    }
    
    let content = hex::encode(&content_bytes);  

    Some((content_type, content))
}

/// Parse script into data chunks
fn parse_script_chunks(script: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut chunks = Vec::new();
    let mut offset = 0;

    while offset < script.len() {
        let opcode = script[offset];

        match opcode {
            // OP_0 pushes empty
            0x00 => {
                chunks.push(Vec::new());
                offset += 1;
            }
            // Direct push (1-75 bytes)
            0x01..=0x4b => {
                let length = opcode as usize;
                offset += 1;
                if offset + length > script.len() {
                    break;
                }
                chunks.push(script[offset..offset + length].to_vec());
                offset += length;
            }
            // OP_PUSHDATA1
            0x4c => {
                offset += 1;
                if offset >= script.len() {
                    break;
                }
                let length = script[offset] as usize;
                offset += 1;
                if offset + length > script.len() {
                    break;
                }
                chunks.push(script[offset..offset + length].to_vec());
                offset += length;
            }
            // OP_PUSHDATA2
            0x4d => {
                offset += 1;
                if offset + 2 > script.len() {
                    break;
                }
                let length = u16::from_le_bytes([script[offset], script[offset + 1]]) as usize;
                offset += 2;
                if offset + length > script.len() {
                    break;
                }
                chunks.push(script[offset..offset + length].to_vec());
                offset += length;
            }
            // OP_PUSHDATA4
            0x4e => {
                offset += 1;
                if offset + 4 > script.len() {
                    break;
                }
                let length = u32::from_le_bytes([
                    script[offset],
                    script[offset + 1],
                    script[offset + 2],
                    script[offset + 3],
                ]) as usize;
                offset += 4;
                if offset + length > script.len() {
                    break;
                }
                chunks.push(script[offset..offset + length].to_vec());
                offset += length;
            }
            // OP_1 to OP_16 push numbers
            0x51..=0x60 => {
                chunks.push(vec![opcode - 0x50]);
                offset += 1;
            }
            // Other opcodes - skip
            _ => {
                offset += 1;
            }
        }
    }

    Some(chunks)
}

/// Parse DAR-20 data from inscription
pub fn parse_dar20(inscription: &Inscription) -> Option<Dar20Inscription> {
    // DAR-20 must be application/json or text/plain
    if !inscription.content_type.contains("json") && !inscription.content_type.contains("text") {
        return None;
    }

    // Decode content
    let content_bytes = hex::decode(&inscription.content).ok()?;
    let content_str = String::from_utf8(content_bytes).ok()?;

    // Try to parse as JSON
    let json: serde_json::Value = serde_json::from_str(&content_str).ok()?;

    // Check for "p": "dar-20" protocol
    let protocol = json.get("p")?.as_str()?;
    if protocol != "dar-20" {
        return None;
    }

    // Get operation
    let op_str = json.get("op")?.as_str()?;
    let operation = match op_str {
        "deploy" => Dar20Operation::Deploy,
        "mint" => Dar20Operation::Mint,
        "transfer" => Dar20Operation::Transfer,
        _ => return None,
    };

    // Get tick (required)
    let tick = json.get("tick")?.as_str()?.to_uppercase();

    // Get optional fields
    let max = json.get("max").and_then(|v| v.as_str()).map(String::from);
    let lim = json.get("lim").and_then(|v| v.as_str()).map(String::from);
    let amt = json.get("amt").and_then(|v| v.as_str()).map(String::from);
    let dec = json
        .get("dec")
        .and_then(|v| v.as_u64())
        .map(|v| v as u8)
        .or(Some(18)); // Default to 18 decimals

    Some(Dar20Inscription {
        protocol: protocol.to_string(),
        operation,
        tick,
        max,
        lim,
        amt,
        dec,
    })
}

/// Get first output address from transaction (usually the inscription holder)
pub fn get_first_output_address(tx: &RawTransaction) -> Option<String> {
    tx.vout.first().and_then(|output| {
        output
            .script_pub_key
            .address
            .clone()
            .or_else(|| output.script_pub_key.addresses.as_ref()?.first().cloned())
    })
}

/// Get sender address from transaction (from input's previous output)
#[allow(dead_code)]
pub fn get_sender_address(tx: &RawTransaction) -> Option<String> {
    // For reveal transactions, we need to look at the commit transaction
    // For simplicity, we use the first output address as both sender/receiver
    // In production, you'd trace the UTXO chain
    get_first_output_address(tx)
}

/// Parse DashDomain inscription (format: "name.dash")
/// Example: "alice.dash" registers domain "alice"
pub fn parse_dash_domain(inscription: &Inscription) -> Option<DashDomainInscription> {
    // DashDomain must be text/plain
    if !inscription.content_type.contains("text") {
        return None;
    }

    // Decode content
    let content_bytes = hex::decode(&inscription.content).ok()?;
    let content_str = String::from_utf8(content_bytes).ok()?;
    let content_trimmed = content_str.trim().to_lowercase();

    // Check for .dash suffix (but not .dashmap)
    if !content_trimmed.ends_with(".dash") || content_trimmed.ends_with(".dashmap") {
        return None;
    }

    // Extract domain name (everything before .dash)
    let name = content_trimmed.strip_suffix(".dash")?;

    // Validate domain name
    if !validate_domain_name(name) {
        return None;
    }

    debug!("Found DashDomain registration: {}.dash", name);

    Some(DashDomainInscription {
        name: name.to_string(),
        full_name: content_trimmed,
    })
}

/// Validate domain name rules
/// - Length: 1-64 characters
/// - Allowed: a-z, 0-9, - (hyphen)
/// - Cannot start or end with hyphen
/// - Cannot be only numbers
pub fn validate_domain_name(name: &str) -> bool {
    let len = name.len();
    
    // Length check
    if len < 1 || len > 64 {
        return false;
    }

    // Cannot start or end with hyphen
    if name.starts_with('-') || name.ends_with('-') {
        return false;
    }

    // Only allowed characters: a-z, 0-9, -
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return false;
    }

    // Cannot be only numbers (to avoid confusion with block numbers)
    if name.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }

    true
}

/// Parse DashMap inscription (format: "blockheight.dashmap")
/// Example: "2386000.dashmap" claims block 2386000
pub fn parse_dashmap(inscription: &Inscription) -> Option<DashMapInscription> {
    // DashMap must be text/plain
    if !inscription.content_type.contains("text") {
        return None;
    }

    // Decode content
    let content_bytes = hex::decode(&inscription.content).ok()?;
    let content_str = String::from_utf8(content_bytes).ok()?;
    let content_trimmed = content_str.trim();

    // Check for .dashmap suffix
    if !content_trimmed.to_lowercase().ends_with(".dashmap") {
        return None;
    }

    // Extract block height (everything before .dashmap)
    let block_str = content_trimmed
        .strip_suffix(".dashmap")
        .or_else(|| content_trimmed.strip_suffix(".DASHMAP"))
        .or_else(|| content_trimmed.strip_suffix(".DashMap"))?;

    // Parse block height
    let block_height: u64 = block_str.trim().parse().ok()?;

    debug!("Found DashMap claim for block {}", block_height);

    Some(DashMapInscription { block_height })
}

/// Validate DAR-20 ticker (exactly 4 ASCII characters, alphanumeric)
/// Canonical BRC-20 rule: tick must be exactly 4 ASCII chars (case-insensitive)
pub fn validate_ticker(tick: &str) -> bool {
    tick.len() == 4 && tick.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Parse amount string to u128 (for precision math)
pub fn parse_amount(amount: &str, decimals: u8) -> Option<u128> {
    let parts: Vec<&str> = amount.split('.').collect();
    
    match parts.len() {
        1 => {
            // Integer only
            let integer: u128 = parts[0].parse().ok()?;
            let multiplier = 10u128.pow(decimals as u32);
            integer.checked_mul(multiplier)
        }
        2 => {
            // Has decimal part
            let integer: u128 = parts[0].parse().ok()?;
            let decimal_str = parts[1];
            
            if decimal_str.len() > decimals as usize {
                return None; // Too many decimal places
            }
            
            let decimal: u128 = decimal_str.parse().ok()?;
            let decimal_places = decimal_str.len() as u32;
            
            let multiplier = 10u128.pow(decimals as u32);
            let decimal_multiplier = 10u128.pow(decimals as u32 - decimal_places);
            
            let integer_part = integer.checked_mul(multiplier)?;
            let decimal_part = decimal.checked_mul(decimal_multiplier)?;
            
            integer_part.checked_add(decimal_part)
        }
        _ => None,
    }
}

/// Format u128 amount to string with decimals
pub fn format_amount(amount: u128, decimals: u8) -> String {
    let divisor = 10u128.pow(decimals as u32);
    let integer = amount / divisor;
    let decimal = amount % divisor;
    
    if decimal == 0 {
        integer.to_string()
    } else {
        let decimal_str = format!("{:0>width$}", decimal, width = decimals as usize);
        let trimmed = decimal_str.trim_end_matches('0');
        format!("{}.{}", integer, trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_ticker() {
        // Canonical BRC-20: tick must be exactly 4 ASCII characters
        assert!(validate_ticker("DASH"));
        assert!(validate_ticker("ORDI"));
        assert!(validate_ticker("ABCD"));
        assert!(!validate_ticker("D20")); // Too short (3 chars)
        assert!(!validate_ticker("A")); // Too short (1 char)
        assert!(!validate_ticker("ABCDEF")); // Too long (6 chars)
        assert!(!validate_ticker("ABCDEFG")); // Too long (7 chars)
        assert!(!validate_ticker("")); // Too short (0 chars)
        assert!(!validate_ticker("DASH!")); // Invalid char
        assert!(!validate_ticker("ABC")); // Too short (3 chars)
        assert!(!validate_ticker("ABCDE")); // Too long (5 chars)
    }

    #[test]
    fn test_parse_amount() {
        assert_eq!(parse_amount("100", 18), Some(100_000_000_000_000_000_000u128));
        assert_eq!(parse_amount("1.5", 18), Some(1_500_000_000_000_000_000u128));
        assert_eq!(parse_amount("0.000001", 18), Some(1_000_000_000_000u128));
    }

    #[test]
    fn test_format_amount() {
        assert_eq!(format_amount(100_000_000_000_000_000_000u128, 18), "100");
        assert_eq!(format_amount(1_500_000_000_000_000_000u128, 18), "1.5");
        assert_eq!(format_amount(1_000_000_000_000u128, 18), "0.000001");
    }
}

