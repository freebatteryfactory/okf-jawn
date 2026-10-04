set shell := ["bash", "-cu"]

default:
    @just --list

init:
    node scripts/dev.mjs init

# Source/config diagnostics that require no downloaded packages.
doctor:
    node scripts/dev.mjs doctor

# Explicit first resolution. Does not invent a Cargo.lock or silently update a selected pin.
bootstrap:
    node scripts/dev.mjs bootstrap

gen:
    node scripts/dev.mjs gen

gen-check:
    node scripts/dev.mjs gen-check

check:
    node scripts/dev.mjs check

check-offline:
    node scripts/dev.mjs check-offline

test:
    node scripts/dev.mjs test

# Query vendor locations and concrete symbols without requiring this conversation.
vendor query="":
    node scripts/dev.mjs vendor "{{query}}"

# Run generator + consumer checks twice and compare the complete generated file sets.
foundation:
    node scripts/dev.mjs foundation

# Produce the complete actual repository tree.
tree:
    node scripts/dev.mjs tree

# Create isolated Git worktrees from an already committed foundation.
lanes:
    node scripts/dev.mjs lanes

# Qualification uses a real implementation supplied by the corresponding lane.
qualify case="":
    node scripts/dev.mjs qualify "{{case}}"
