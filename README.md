# dash-ord

**Dash Ordinals Indexer**

A high-performance, Rust-based indexer for Ordinals-style inscriptions and related protocols on the **Dash** blockchain.

This project indexes digital artifacts inscribed on Dash satoshis (similar to Bitcoin Ordinals) and supports emerging token standards on Dash (e.g. **DAR-20** or equivalent).

> **Status:** Active development. APIs, features, and documentation are subject to change.

---

## Features

- Real-time Dash blockchain indexing
- Ordinals-style inscription detection and parsing
- Inscription metadata and content storage
- SQLite database backend (default)
- High-performance, memory-safe Rust implementation
- Database backup support

---

## Requirements

- Rust (edition 2021 or newer)
- Cargo
- A fully synced Dash node (`dashd`)
- Dash RPC enabled with credentials

---

## Installation

### Clone the Repository

```
git clone https://github.com/DashyMarket/dash-ord.git
cd dash-ord
```

### Build

```
cargo build --release
```

### Binary Location

The compiled binary will be located at:

```
target/release/
```

The binary name is defined in `Cargo.toml` (typically `dash-ord`).

---

## Configuration

Configuration is handled via environment variables or configuration files (refer to source code for full options).

### Required Environment Variables

- `DASH_RPC_URL`
- `DASH_RPC_USER`
- `DASH_RPC_PASS`
- `DATABASE_PATH` (optional, defaults to `dashymarket_indexer.db`)

### Example

```
export DASH_RPC_URL=http://127.0.0.1:9998
export DASH_RPC_USER=rpcuser
export DASH_RPC_PASS=rpcpassword
```

---

## Usage

### Start Indexing (From Chain Tip)

```
./target/release/dash-ord index
```

### Reindex From Genesis or Specific Height

```
./target/release/dash-ord reindex --from-height 0
```

### Show Help

```
./target/release/dash-ord --help
```

---

## Database

- Backend: SQLite
- Default database file: `dashymarket_indexer.db`
- Stores:
  - Blocks
  - Transactions
  - Inscriptions
  - Satoshi ranges
  - Metadata
- Optional backups stored in the `backups/` directory

---

## Contributing

Contributions are welcome.

You can help by:

- Opening issues for bugs or feature requests
- Submitting pull requests
- Improving documentation
- Adding tests
- Optimizing performance

Please follow Rust best practices and include tests where applicable.

---

## License

This project is open source.

A license file will be added.  
Recommended licenses:

- MIT
- Apache-2.0

---

## Acknowledgments

- Inspired by the Bitcoin Ordinals protocol
- Built specifically for the Dash ecosystem
- Developed by **DashyMarket**

---

## Support

For questions, bugs, or feature requests, please open an issue on this repository.
