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

# Create isolated Git worktrees from an already committed foundation.
lanes:
    bun scripts/dev.mjs lanes

# Qualification uses a real implementation supplied by the corresponding lane.
qualify case="":
    bun scripts/dev.mjs qualify "{{case}}"
