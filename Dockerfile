FROM rust:1.96-bookworm AS build

WORKDIR /app
COPY . .
RUN cargo build --release --locked

FROM debian:bookworm-slim

LABEL org.opencontainers.image.source="https://github.com/bapho-bush/bugboard-mcp"
LABEL org.opencontainers.image.description="MCP server for the 1C Bugboard"

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home app

COPY --from=build /app/target/release/bugboard-mcp /usr/local/bin/bugboard-mcp
USER app
ENTRYPOINT ["bugboard-mcp"]
CMD ["--stdio"]
