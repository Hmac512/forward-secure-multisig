.PHONY: help
help:
	@echo "Setup forward secure signatures"
	@echo ""
	@echo	"Usage:"
	@echo "make build:                                    --- initalizes and builds"
	@echo "make clean:                                    --- cleans build"
	@echo "make test:                                     --- run unit tests"
	@echo "make benchmark:                                --- run unit tests w/ timer"

.PHONY: build
build:
	cargo build

.PHONY: clean
clean:
	cargo clean

.PHONY: test
test:
	cargo test

.PHONY: benchmark
benchmark:
	RUST_TEST_THREADS=1 cargo test --no-default-features --features VerkeyG2 -- --nocapture timing


