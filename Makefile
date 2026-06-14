# Detect docker compose command (v2 vs v1)
DOCKER_COMPOSE := $(shell if docker compose version >/dev/null 2>&1; then echo "docker compose"; else echo "docker-compose"; fi)

# Default passwords for Docker test connections
export SEXTANT_DOCKER_PG_PASSWORD ?= sextant
export SEXTANT_DOCKER_MYSQL_PASSWORD ?= sextant

# Default connection URLs pointing to Docker Compose services
export SEXTANT_TEST_PG_URL ?= postgres://sextant:sextant@localhost:5433/sextant_test
export SEXTANT_TEST_MYSQL_URL ?= mysql://sextant:sextant@localhost:3307/sextant_test

.PHONY: test-db-up test-db-down test-integration test-db seed seed-sqlite help check e2e smoke

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

check: ## Full verification: compile, test, fmt check, clippy
	cargo check --workspace
	cargo test --workspace
	cargo fmt --all --check
	cargo clippy --workspace --all-targets

e2e: ## Run the PTY end-to-end tests (SQLite always; PG/MySQL when Docker is available)
	cargo test -p sextant-cli --test e2e
	cargo test -p sextant-cli --test e2e_drivers

smoke: ## Print live "screenshots" of the running TUI (manual, no TTY needed)
	cargo test -p sextant-cli --test smoke -- --ignored --nocapture

test-db-up: ## Start PostgreSQL and MySQL test containers
	$(DOCKER_COMPOSE) up -d
	@echo "Waiting for databases to be healthy..."
	@until $(DOCKER_COMPOSE) ps postgres | grep -q "healthy"; do sleep 1; done
	@until $(DOCKER_COMPOSE) ps mysql | grep -q "healthy"; do sleep 1; done
	@echo "Databases are ready!"

test-db-down: ## Stop and remove test containers and volumes
	$(DOCKER_COMPOSE) down -v

test-integration: ## Run integration tests against Docker databases (PG/MySQL)
	cargo test --workspace

test-db: test-db-up test-integration test-db-down ## Full cycle: start DBs, run tests, tear down

seed: seed-sqlite ## Seed all test databases (PostgreSQL, MySQL, SQLite)
	@echo "Seeding PostgreSQL..."
	@docker exec -i sextant-postgres-test psql -U sextant -d sextant_test -q < seeds/postgres.sql
	@echo "Seeding MySQL..."
	@docker exec -i sextant-mysql-test mysql -u sextant -psextant sextant_test < seeds/mysql.sql
	@echo "All seeds applied."

seed-sqlite: ## Seed local SQLite test.db
	@echo "Seeding SQLite..."
	@sqlite3 test.db < seeds/sqlite.sql
	@echo "SQLite seeded."

setup-docker-conns: ## Install Docker test connections to ~/.config/sextant/
	@mkdir -p ~/.config/sextant
	@cp connections.example.toml ~/.config/sextant/connections.toml
	@echo "Installed connections to ~/.config/sextant/connections.toml"
	@echo "Set passwords with:"
	@echo "  export SEXTANT_DOCKER_PG_PASSWORD=sextant"
	@echo "  export SEXTANT_DOCKER_MYSQL_PASSWORD=sextant"
