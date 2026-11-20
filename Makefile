DC = docker compose
.PHONY: help dev-init build up down logs test test-unit test-integration test-s3 lint fmt clean
help:
	@echo "dev-init build up down logs test test-unit test-integration test-s3 lint fmt clean"
dev-init:
	docker compose -f compose.bootstrap.yml run --rm initialize
build:
	$(DC) build datavault cli integration-tests
up:
	$(DC) up -d --wait
down:
	$(DC) down
logs:
	$(DC) logs -f datavault
test: test-unit test-integration
test-unit:
	$(DC) run --rm --build rust cargo test --locked --workspace
test-integration:
	$(DC) run --rm postgres-tests
	$(DC) run --rm --build integration-tests
test-s3:
	$(DC) run --rm --build integration-tests pytest -q -p no:cacheprovider -k aws_cli
lint:
	$(DC) run --rm --build rust cargo clippy --locked --workspace --all-targets --all-features
fmt:
	$(DC) run --rm --build rust cargo fmt --all
clean:
	@test "$(CONFIRM)" = "delete-development-volumes" || (echo "Use CONFIRM=delete-development-volumes to erase development volumes"; exit 1)
	$(DC) down -v
