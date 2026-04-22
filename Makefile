# Top-level Makefile — delegates to nf-server and subcrates
#
# Storage: flat binary files (no SQLite).
#
#   nullifiers.bin         – append-only raw 32-byte nullifier blobs
#   nullifiers.checkpoint  – 16-byte (height LE, offset LE) crash-recovery marker
#   nullifiers.index       – height → byte offset index
#   nullifiers.tree        – v1 bincode PIR Merkle checkpoint (SVOTEPT1 magic)
#   pir-data/              – PIR tier files (tier0.bin, tier1.bin, tier2.bin, pir_root.json)
#
# Pipeline: `make sync` → `make serve`
# ──────────────────────────────────
# `make sync` runs `nf-server sync` (nullifiers from lightwalletd → tree checkpoint → tiers).
# Empty `SVOTE_VOTING_CONFIG_URL` skips voting height cap / prompts.
# `SVOTE_PIR_SYNC_RESET=1` wipes nullifiers + tree + tiers before a run.
# `make sync-invalidate` passes `--invalidate-after-blocks` (rebuild tree + tiers when new blocks were synced).

IMT_DIR     := imt-tree
SERVICE_DIR := nf-ingest
NF_DIR      := nf-server

# ── Configuration (override with env vars) ───────────────────────────
DATA_DIR      ?= .
LWD_URL       ?= https://zec.rocks:443
PORT          ?= 3000
SYNC_HEIGHT   ?=
PIR_DATA_DIR  ?= $(DATA_DIR)/pir-data

# Validate SYNC_HEIGHT and build --max-height for `nf-server sync`.
ifdef SYNC_HEIGHT
  ifneq ($(shell expr $(SYNC_HEIGHT) % 10),0)
    $(error SYNC_HEIGHT must be a multiple of 10, got $(SYNC_HEIGHT))
  endif
  _MAX_HEIGHT_FLAG := --max-height $(SYNC_HEIGHT)
else
  _MAX_HEIGHT_FLAG :=
endif

_SYNC_CMD := cd $(NF_DIR) && cargo run --release -- sync --data-dir ../$(DATA_DIR) --output-dir ../$(PIR_DATA_DIR) --lwd-url $(LWD_URL) $(_MAX_HEIGHT_FLAG)

# ── Targets ──────────────────────────────────────────────────────────

.PHONY: build-nf sync sync-invalidate serve build test clean status help

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'

build-nf: ## Build nf-server binary (release, nightly)
	cd $(NF_DIR) && cargo build --release

build: ## Build nf-server and service library (release)
	cd $(NF_DIR) && cargo build --release

sync: ## `nf-server sync`: nullifiers + tree checkpoint + PIR tiers (resumable)
	$(_SYNC_CMD)

sync-invalidate: ## Same as sync with `--invalidate-after-blocks` (rebuild tree/tiers when new blocks synced)
	cd $(NF_DIR) && cargo run --release -- sync --data-dir ../$(DATA_DIR) --output-dir ../$(PIR_DATA_DIR) --lwd-url $(LWD_URL) --invalidate-after-blocks $(_MAX_HEIGHT_FLAG)

serve: ## Start the PIR HTTP server
	cd $(NF_DIR) && cargo run --release --features serve -- serve --pir-data-dir ../$(PIR_DATA_DIR) --data-dir ../$(DATA_DIR) --port $(PORT)

test: ## Run unit tests for all subcrates
	cd $(IMT_DIR) && cargo test --lib
	cd $(SERVICE_DIR) && cargo test --lib

status: ## Show nullifier sync progress (count + checkpoint + tree file)
	@NF="$(DATA_DIR)/nullifiers.bin"; CP="$(DATA_DIR)/nullifiers.checkpoint"; \
	TREE="$(DATA_DIR)/nullifiers.tree"; \
	echo "Data directory: $(DATA_DIR)"; \
	if [ -f "$$NF" ]; then \
		SIZE=$$(ls -lh "$$NF" | awk '{print $$5}'); \
		BYTES=$$(wc -c < "$$NF" | tr -d ' '); \
		COUNT=$$((BYTES / 32)); \
		echo "  nullifiers.bin: $$COUNT nullifiers ($$SIZE)"; \
	else \
		echo "  nullifiers.bin: not found"; \
	fi; \
	if [ -f "$$CP" ]; then \
		HEIGHT=$$(od -An -t u8 -j 0 -N 8 "$$CP" | tr -d ' '); \
		OFFSET=$$(od -An -t u8 -j 8 -N 8 "$$CP" | tr -d ' '); \
		echo "  checkpoint: height=$$HEIGHT offset=$$OFFSET"; \
	else \
		echo "  checkpoint: none"; \
	fi; \
	if [ -f "$$TREE" ]; then \
		TSIZE=$$(ls -lh "$$TREE" | awk '{print $$5}'); \
		echo "  nullifiers.tree: $$TSIZE (PIR tree checkpoint)"; \
	else \
		echo "  nullifiers.tree: not present"; \
	fi

clean: ## Remove built artifacts and data files
	cd $(IMT_DIR) && cargo clean
	cd $(SERVICE_DIR) && cargo clean
	cd $(NF_DIR) && cargo clean
	rm -f $(DATA_DIR)/nullifiers.bin $(DATA_DIR)/nullifiers.checkpoint $(DATA_DIR)/nullifiers.index $(DATA_DIR)/nullifiers.tree $(DATA_DIR)/nullifiers.tree.tmp
