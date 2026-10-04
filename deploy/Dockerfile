# The foundation target can build after resolution; the application target requires
# the server lane to add the real okf-jawn-server binary. No dummy server is shipped.
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
RUN cargo build --locked --release --package okf-jawn-server --bin okf-jawn-server
FROM ${RUNTIME_IMAGE} AS application
WORKDIR /app
COPY --from=application-build /workspace/target/release/okf-jawn-server /app/okf-jawn-server
COPY --from=application-build /workspace/ui/dist /app/ui
COPY --from=application-build /workspace/ui/dist-apps /app/mcp-apps
COPY --from=application-build /workspace/api /app/api
ENV OKF_BIND=0.0.0.0:7711 OKF_DATA_DIR=/data
USER 65532:65532
EXPOSE 7711
VOLUME ["/data"]
ENTRYPOINT ["/app/okf-jawn-server"]
