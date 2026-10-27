.PHONY: run dev test live-test check app install

run:            ## the app, release build
	cargo run --release

dev:            ## debug build, tokens in a file instead of the Keychain
	AYEAYE_TOKEN_STORE=file cargo run

test:
	cargo test

live-test:      ## against a real instance: AYEAYE_TEST_SERVER/EMAIL/PASSWORD
	cargo test live_ -- --ignored --nocapture

check:
	cargo fmt --check && cargo clippy --features devtools -- -D warnings && cargo test

app:            ## dist/Ayeaye.app (macOS)
	script/bundle-macos.sh

install: app    ## copy it to /Applications, the copy Spotlight and the Dock open
	rm -rf /Applications/Ayeaye.app && cp -R dist/Ayeaye.app /Applications/
