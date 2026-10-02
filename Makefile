.PHONY: test check e2e package

test:
	cargo test --all-targets
	python3 scripts/test-dircount.py

check:
	cargo fmt --all -- --check
	cargo clippy --all-targets --all-features -- -D warnings
	cargo test --all-targets
	python3 scripts/test-dircount.py

e2e:
	cargo build --release
	bash scripts/test-e2e.sh

package:
	bash scripts/package.sh
