SHELL := /bin/bash

.PHONY: setup dev build wasm dashboard check docker clean deploy deploy-observability

setup:
	./scripts/setup-dev.sh

dev:
	cargo run -p temperpaw

build:
	cd dashboard && npm run build
	cargo build -p temperpaw --release

wasm:
	@for script in os-apps/*/wasm/build.sh os-apps/*/wasm/*/build.sh; do \
		[ -f "$$script" ] || continue; \
		echo "==> $$script"; \
		(cd "$$(dirname "$$script")" && bash ./build.sh) || exit 1; \
	done

dashboard:
	cd dashboard && npm run dev

check:
	cargo test -p temperpaw --quiet
	cd dashboard && npm run build

docker:
	docker build -t temperpaw:dev .

clean:
	cargo clean
	rm -rf dashboard/build dashboard/.svelte-kit

deploy:
	cargo run -p temperpaw -- deploy

deploy-observability:
	python3 scripts/deploy_dashboard.py
	python3 scripts/deploy_monitors.py
