.PHONY: test check e2e package

test:
	cargo test --all-targets

check:
	cargo fmt --all -- --check
	cargo clippy --all-targets --all-features -- -D warnings
	cargo test --all-targets

e2e:
	cargo build --release
	python3 tests/integration/test-everything.py target/release/nasfind
	bash tests/integration/test-e2e.sh

package:
	bash scripts/package.sh
