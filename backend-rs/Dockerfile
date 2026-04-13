FROM rust:1-bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations

RUN cargo build --release --locked

FROM debian:bookworm-slim AS runner
WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libgcc-s1 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system app \
    && useradd --system --gid app --create-home --home-dir /home/app --shell /usr/sbin/nologin app \
    && mkdir -p /data \
    && chown -R app:app /app /data

COPY --from=builder /app/target/release/sub2apipay-rs /usr/local/bin/sub2apipay-rs
COPY config.example.toml /app/config.example.toml

VOLUME ["/data"]

USER app

EXPOSE 8080

CMD ["sub2apipay-rs"]
