set shell := ["bash", "-cu"]

default:
    @just --list

init:
    bun scripts/dev.mjs init

# Source/config diagnostics that require no downloaded packages.
doctor:
    bun scripts/dev.mjs doctor

# Explicit resolution with the selected Cargo and Bun. Never runs implicitly from another task.
lock:
    bun scripts/dev.mjs lock

# Locked installation, generation, and consumer build. Missing lockfiles are an error.
bootstrap:
    bun scripts/dev.mjs bootstrap

gen:
    bun scripts/dev.mjs gen

gen-check:
    bun scripts/dev.mjs gen-check

check:
    bun scripts/dev.mjs check

check-offline:
    bun scripts/dev.mjs check-offline

test:
    bun scripts/dev.mjs test

# Query vendor locations and concrete symbols without requiring this conversation.
vendor query="":
    bun scripts/dev.mjs vendor "{{query}}"

# Run generator + consumer checks twice and compare the complete generated file sets.
foundation:
    bun scripts/dev.mjs foundation

# Produce the complete actual repository tree.
tree:
    bun scripts/dev.mjs tree

# Create isolated Git worktrees from an already committed foundation (all lanes, or the named ones).
lanes *names:
    bun scripts/dev.mjs lanes {{names}}

# Qualification uses a real implementation supplied by the corresponding lane.
qualify case="":
    bun scripts/dev.mjs qualify "{{case}}"

# Strict cargo-audit plus bun audit --prod and --audit-level=high. Full bun audit is recorded only.
audit:
    bun scripts/dev.mjs audit

# Remove clean lane worktrees and branches that hold no commits. Never forces.
lanes-reset:
    bun scripts/dev.mjs lanes-reset

# One lane's gate: fmt, Clippy, tests, source policy, scope (UI lanes: Biome, routes, tsc, Vitest).
lane name:
    bun scripts/dev.mjs lane "{{name}}"

# The CI sequence with every feature; runs every step and reports all failures.
premerge:
    bun scripts/dev.mjs premerge

check-receipts:
    bun scripts/dev.mjs check-receipts

# The whole foundation in a temporary detached worktree of HEAD; writes a receipt under .artifacts/.
clean-checkout:
    bun scripts/dev.mjs clean-checkout
