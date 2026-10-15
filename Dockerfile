# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p pitwall && cp target/release/pitwall /pitwall

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /app pitwall \
    && mkdir -p /data && chown pitwall /data
WORKDIR /app
COPY --from=build /pitwall /usr/local/bin/pitwall
# Formats and cups ship with the image; a deployment without pitwall.toml configures the rest
# through PITWALL_* variables (see deploy/portainer/stack.yml).
COPY config /app/config
USER pitwall
ENV PITWALL_CONFIG=/app/pitwall.toml
EXPOSE 8080
ENTRYPOINT ["pitwall"]
