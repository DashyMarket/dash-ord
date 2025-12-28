# dash-ord

**Dash Ordinals Indexer**

A high-performance, Rust-based indexer for Ordinals-style inscriptions and related protocols on the **Dash** blockchain.

This project tracks digital artifacts inscribed on Dash satoshis (similar to Bitcoin Ordinals) and supports emerging token standards on Dash (e.g., DAR-20 or equivalent).

> **Note:** This repository is under active development. Features and documentation will evolve as the project matures.

## Features

- Real-time indexing of the Dash blockchain
- Detection and parsing of Ordinals inscriptions
- Storage of inscription metadata and content
- SQLite database backend (default)
- Efficient, memory-safe implementation in Rust
- Backup support for database safety

## Prerequisites

- Rust (2021 edition or later) and Cargo
- A running Dash node (`dashd`) with RPC enabled
- Access to the full Dash blockchain

## Installation

1. Clone the repository:
   ```bash
   git clone https://github.com/DashyMarket/dash-ord.git
   cd dash-ord
	2	Build the project: cargo build --release
	3	 The executable will be located at target/release/ (binary name depends on Cargo.toml – typically dash-ord or similar).
Configuration
Configuration is primarily handled via environment variables or a config file (check the source code for exact details).
Common required settings:
	•	Dash RPC URL, username, and password
	•	Database path (defaults to SQLite file dashymarket_indexer.db)
Example:
export DASH_RPC_URL="http://localhost:9998"
export DASH_RPC_USER="your_rpc_user"
export DASH_RPC_PASS="your_rpc_password"
Usage
Basic commands (adjust based on actual binary name and available subcommands):
# Start indexing from the current chain tip
./target/release/dash-ord index

# Reindex from genesis or a specific height
./target/release/dash-ord reindex --from-height 0

# Show help for all available commands
./target/release/dash-ord --help
Database
	•	Default: SQLite database file dashymarket_indexer.db
	•	Stores blocks, inscriptions, satoshi ranges, and metadata
	•	Database backups can be stored in the backups/ directory
Contributing
Contributions are welcome! Feel free to:
	•	Open issues for bugs or feature requests
	•	Submit pull requests with improvements
	•	Enhance documentation
	•	Add tests or optimize performance
Please adhere to Rust coding conventions and include tests where applicable.
License
This project is open source. A LICENSE file will be added soon (recommended: MIT or Apache 2.0).
Acknowledgments
	•	Inspired by the Bitcoin Ordinals protocol and existing indexers
	•	Built specifically for the Dash ecosystem by DashyMarket
For support or questions, please open an issue on this repository.

