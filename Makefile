COMPOSE := docker compose --profile rust
RUN := $(COMPOSE) run --rm rust
DB := $(COMPOSE) exec -T postgis
PGUSER := $(or $(POSTGRES_USER),atlas)
PGDB := $(or $(POSTGRES_DB),atlas)
BACKUPS := backups
FILE ?= $(lastword $(sort $(wildcard $(BACKUPS)/*.dump)))

export ATLAS_UID ?= $(shell id -u)
export ATLAS_GID ?= $(shell id -g)

.PHONY: build fmt lint test check migrate up serve logs restart down shell backup restore

build:
	$(COMPOSE) build rust

fmt:
	$(RUN) cargo fmt --check

lint:
	$(RUN) cargo clippy --locked --all-targets --all-features -- -D warnings

test:
	$(RUN) cargo test --locked

check: fmt lint test

migrate:
	$(RUN) cargo run --locked -- migrate

up:
	$(COMPOSE) up -d --build atlas
	@echo "Atlas en http://127.0.0.1:$${ATLAS_PORT:-8080}/api/v1/health (make logs)"

serve:
	$(COMPOSE) up --build atlas

logs:
	$(COMPOSE) logs -f atlas

restart:
	$(COMPOSE) restart atlas

down:
	$(COMPOSE) down

shell:
	$(RUN) bash

backup:
	@mkdir -p $(BACKUPS)
	$(DB) pg_dump -U $(PGUSER) -d $(PGDB) -Fc > $(BACKUPS)/$(PGDB)-$$(date +%Y%m%d-%H%M%S).dump
	@ls -1t $(BACKUPS)/*.dump | head -1

restore:
	@test -n "$(FILE)" || { echo "No hay respaldo en $(BACKUPS)/; usa FILE=ruta.dump"; exit 1; }
	@echo "Restaurando $(FILE) sobre $(PGDB)"
	$(DB) pg_restore -U $(PGUSER) -d $(PGDB) --clean --if-exists --no-owner --single-transaction < $(FILE)
