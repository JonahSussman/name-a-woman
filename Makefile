.PHONY: types check fmt dev release serve

## Generate TS types from Rust structs
types:
	cargo test export_bindings

## Type-check frontend and backend
check: types
	cargo check
	cd frontend && npx tsc

## Format all code
fmt:
	cargo fmt
	cd frontend && npx prettier --write app.ts

## Build frontend and backend (dev)
dev: types
	mkdir -p static
	cd frontend && npx esbuild app.ts --bundle --outfile=../static/app.js
	cp frontend/style.css frontend/index.html static/
	cargo build

## Build frontend (minified) and backend (release)
release: check
	mkdir -p static
	cd frontend && npx esbuild app.ts --bundle --minify --outfile=../static/app.js
	cd frontend && npx esbuild style.css --minify --outfile=../static/style.css
	cd frontend && npx html-minifier-terser --collapse-whitespace --remove-comments --minify-css --minify-js -o ../static/index.html index.html
	cargo build --release

## Build dev and run server
serve: dev
	cargo run --bin name-a-woman
