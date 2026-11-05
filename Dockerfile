ARG RUST_VERSION=1.99.1

ARG APP_NAME=federated-blog

FROM docker.io/library/rust:${RUST_VERSION}-alpine AS build

ARG APP_NAME

WORKDIR /app

RUN apk add --no-cache clang lld musl-dev git

RUN --mount=type=bind,source=src,target=src \
    --mount=type=bind,source=Cargo.toml,target=Cargo.toml \
    --mount=type=bind,source=Cargo.lock,target=Cargo.lock \
    --mount=type=bind,source=migrations,target=migrations \
    --mount=type=cache,target=/app/target/ \
    --mount=type=cache,target=/var/cache/cargo \
    CARGO_HOME=/var/cache/cargo cargo build --locked --release && \
    cp ./target/release/federated-blog /bin/federated-blog

FROM docker.io/library/alpine:3.18 AS final

ARG UID=10001
RUN adduser \
    --disabled-password \
    --gecos "" \
    --home "/nonexistent" \
    --shell "/sbin/nologin" \
    --no-create-home \
    --uid "${UID}" \
    appuser

RUN mkdir -p /etc/federated-blog /app && \
    chown appuser /app

USER appuser

COPY --from=build /bin/federated-blog /bin/

WORKDIR /app

CMD ["/bin/federated-blog", "/etc/federated-blog/config.json"]