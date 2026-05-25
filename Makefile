# Detect docker compose command (v2 vs v1)
DOCKER_COMPOSE := $(shell if docker compose version >/dev/null 2>&1; then echo "docker compose"; else echo "docker-compose"; fi)

# Default connection URLs pointing to Docker Compose services
export SEXTANT_TEST_PG_URL ?= postgres://sextant:sextant@localhost:5433/sextant_test
export SEXTANT_TEST_MYSQL_URL ?= mysql://sextant:sextant@localhost:3307/sextant_test

.PHONY: test-db-up test-db-down test-integration test-db help

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

test-db-up: ## Start PostgreSQL and MySQL test containers
	$(DOCKER_COMPOSE) up -d
	@echo "Waiting for databases to be healthy..."
	@until $(DOCKER_COMPOSE) ps postgres | grep -q "healthy"; do sleep 1; done
	@until $(DOCKER_COMPOSE) ps mysql | grep -q "healthy"; do sleep 1; done
	@echo "Databases are ready!"

test-db-down: ## Stop and remove test containers and volumes
	$(DOCKER_COMPOSE) down -v

test-integration: ## Run integration tests against Docker databases
	cargo test --workspace

test-db: test-db-up test-integration test-db-down ## Full cycle: start DBs, run tests, tear down
