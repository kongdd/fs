.PHONY: test check e2e package
TARGET ?= $(shell rustc -vV | awk '/^host:/ {print $$2}')

test:
	cargo test --locked --workspace --all-targets

check:
	cargo fmt --all -- --check
	cargo clippy --locked --workspace --all-targets -- -D warnings
	$(MAKE) test

e2e:
	cargo build --locked --release --bin fs
	python3 tests/integration/test-native.py target/release/fs
	python3 tests/integration/test-everything.py target/release/fs
	python3 tests/integration/test-engine-default.py target/release/fs

package:
	cargo build --locked --release --bin fs --target $(TARGET)
	python3 scripts/release/package.py $(TARGET)
