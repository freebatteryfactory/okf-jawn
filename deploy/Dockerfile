# The application target builds the composed server, CLI and isolated conversion child.
# Runtime native libraries and model assets must be qualified before an image is released.
# Supply digest-pinned images rather than inventing registry hashes in this file.
# BUN_IMAGE must be the oven/bun release selected by .bun-version; doctor rejects any other version.
ARG BUN_IMAGE
ARG RUST_IMAGE
ARG RUNTIME_IMAGE
FROM ${BUN_IMAGE} AS bun-runtime
FROM ${RUST_IMAGE} AS foundation
COPY --from=bun-runtime /usr/local/bin/bun /usr/local/bin/bun
WORKDIR /workspace
COPY . .
RUN bun scripts/dev.mjs bootstrap
RUN bun scripts/dev.mjs foundation
FROM foundation AS application-build
RUN cargo build --locked --release --package okf-jawn-server --features runtime --bin okf-jawn-server
RUN cargo build --locked --release --package okf-jawn-cli --bin okf-jawn
RUN cargo build --locked --release --package okf-jawn-ingest --features runtime --bin okf-jawn-convert
FROM ${RUNTIME_IMAGE} AS application
WORKDIR /app
COPY --from=application-build /workspace/target/release/okf-jawn-server /app/okf-jawn-server
COPY --from=application-build /workspace/target/release/okf-jawn /app/okf-jawn
COPY --from=application-build /workspace/target/release/okf-jawn-convert /app/okf-jawn-convert
COPY --from=application-build /workspace/ui/dist /app/ui
COPY --from=application-build /workspace/ui/dist-apps /app/mcp-apps
ENV OKF_BIND=0.0.0.0:7711 OKF_SANDBOX_BIND=0.0.0.0:7712 OKF_DATA_DIR=/data
USER 65532:65532
EXPOSE 7711 7712
VOLUME ["/data"]
ENTRYPOINT ["/app/okf-jawn-server"]
