# open-au05 task runner

# Format + lint + test — the gate before every commit
check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test

# Run tests
test:
    cargo test

# Build release binary
build:
    cargo build --release

# Format the tree
fmt:
    cargo fmt
