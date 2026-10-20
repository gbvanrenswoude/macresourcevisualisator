.PHONY: app install run test icons clean

app:        ## Build target/Tessera.app
	scripts/build-app.sh

install:    ## Build and install into /Applications, then launch
	scripts/install.sh

run:        ## Run from source (debug build)
	cargo run -p tessera

test:       ## Run the test suite
	cargo test --workspace

icons:      ## Re-render the app icon and menu bar glyph
	cargo run --release -p tessera-icon -- assets
	iconutil -c icns assets/AppIcon.iconset -o assets/AppIcon.icns

clean:
	cargo clean
