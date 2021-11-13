all: test style lint
.PHONY: all

clean:
	@cargo clean
.PHONY: clean

build:
	@cargo build
.PHONY: build

doc:
	@cargo doc --all-features
.PHONY: doc

style:
	@rustup component add rustfmt --toolchain stable 2> /dev/null
	cargo +stable fmt -- --check
.PHONY: style

format:
	@rustup component add rustfmt --toolchain stable 2> /dev/null
	cargo +stable fmt
.PHONY: format

lint:
	@rustup component add clippy --toolchain stable 2> /dev/null
	cargo +stable clippy --all-features --tests --all --examples -- -D clippy::all
.PHONY: lint

test: testall
.PHONY: test

checkall:
	cargo check --all-features
	cargo check --no-default-features
.PHONY: checkall

testall:
	cargo test --all-features
	cargo test --no-default-features --tests
.PHONY: testall

readme:
	cargo readme | perl -p -e "s/\]\(([^\/]+)\)/](https:\/\/docs.rs\/unix-ipc\/latest\/unix_ipc\/\\1)/" > README.md
.PHONY: readme
