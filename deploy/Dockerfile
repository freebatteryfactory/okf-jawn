# The foundation target can build after resolution; the application target requires
# the server lane to add the real okf-jawn-server binary. No dummy server is shipped.
# Supply digest-pinned images rather than inventing registry hashes in this file.
ARG NODE_IMAGE
ARG RUST_IMAGE
ARG RUNTIME_IMAGE
FROM ${NODE_IMAGE} AS node-runtime
FROM ${RUST_IMAGE} AS foundation
COPY --from=node-runtime /usr/local/bin/node /usr/local/bin/node
COPY --from=node-runtime /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -s /usr/local/lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm && \
    npm install --global pnpm@12.8.1
WORKDIR /workspace
COPY . .
RUN node scripts/dev.mjs bootstrap
RUN node scripts/dev.mjs foundation
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
