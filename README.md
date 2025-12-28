# dash-ord

**Dash Ordinals Indexer**

A high-performance, Rust-based indexer for Ordinals-style inscriptions and related protocols on the **Dash** blockchain.

This project tracks digital artifacts inscribed on Dash satoshis (similar to Bitcoin Ordinals) and supports emerging token standards on Dash (e.g., **DAR-20** or equivalent).

> ⚠️ **Note:** This repository is under active development. Features and documentation may change as the project matures.

---

## Features

- Real-time indexing of the Dash blockchain
- Detection and parsing of Ordinals-style inscriptions
- Storage of inscription metadata and content
- SQLite database backend (default)
- Efficient, memory-safe implementation in Rust
- Database backup support

---

## Prerequisites

- Rust (2021 edition or later) and Cargo
- A running Dash node (`dashd`) with RPC enabled
- Access to the full Dash blockchain

---

## Installation

1. **Clone the repository**
   ```bash
   git clone https://github.com/DashyMarket/dash-ord.git
   cd dash-ord

	2.	Build the project

cargo build --release


	3.	Locate the binary
The compiled executable will be located in:

target/release/

The binary name depends on Cargo.toml (typically dash-ord).

⸻

Configuration

Configuration is handled via environment variables or a config file (refer to the source code for exact options).

Required Settings
	•	Dash RPC URL
	•	Dash RPC username
	•	Dash RPC password
	•	Database path (defaults to dashymarket_indexer.db)

Example

export DASH_RPC_URL="http://localhost:9998"
export DASH_RPC_USER="your_rpc_user"
export DASH_RPC_PASS="your_rpc_password"


⸻

Usage

Basic commands (adjust if the binary name differs):

# Start indexing from the current chain tip
./target/release/dash-ord index

# Reindex from genesis or a specific block height
./target/release/dash-ord reindex --from-height 0

# Display help and available commands
./target/release/dash-ord --help


⸻

Database
	•	Default: SQLite database (dashymarket_indexer.db)
	•	Stores:
	•	Blocks
	•	Inscriptions
	•	Satoshi ranges
	•	Metadata
	•	Database backups can be stored in the backups/ directory

⸻

Contributing

Contributions are welcome! You can help by:
	•	Opening issues for bugs or feature requests
	•	Submitting pull requests with improvements
	•	Enhancing documentation
	•	Adding tests or optimizing performance

Please follow Rust coding conventions and include tests where applicable.

⸻

License

This project is open source.

A LICENSE file will be added soon.
Recommended licenses: MIT or Apache-2.0

⸻

Acknowledgments
	•	Inspired by the Bitcoin Ordinals protocol and existing indexers
	•	Built specifically for the Dash ecosystem by DashyMarket

⸻

Support

For questions, issues, or feature requests, please open an issue in this repository.

