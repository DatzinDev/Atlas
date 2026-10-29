FROM rust:1.99.1-slim-trixie@sha256:b8e062b1c3a80183df83d9ba187cadd2b5ab84bc78d453dbfeb2fccd5f982247 AS dev

RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake \
    && rm -rf /var/lib/apt/lists/*

ENV CARGO_TARGET_DIR=/target
WORKDIR /work/rust

COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
RUN rustup show active-toolchain \
    && mkdir src \
    && printf 'fn main() {}\n' > src/main.rs \
    && touch src/lib.rs \
    && cargo fetch --locked \
    && rm -rf src Cargo.toml Cargo.lock rust-toolchain.toml \
    && mkdir -p "$CARGO_TARGET_DIR" "$CARGO_HOME/git" \
    && chmod 1777 "$CARGO_TARGET_DIR" \
    && chmod -R a+rwX "$CARGO_HOME/registry" "$CARGO_HOME/git"

CMD ["cargo", "test", "--locked"]
