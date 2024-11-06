FROM rust:1.82 AS chef
WORKDIR /usr/src/app
RUN cargo install cargo-chef --locked

FROM chef AS planner
COPY Cargo.toml Cargo.lock .
RUN cargo chef prepare --recipe-path recipe.json


FROM chef AS builder
COPY --from=planner /usr/src/app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY ./src ./src
RUN cargo install --path .

FROM debian:bookworm-slim AS db-prep
RUN apt-get update && apt-get install -y git sqlite3 jq && rm -rf /var/lib/apt/lists/*
WORKDIR /usr/src/app
COPY ./scripts/update_rotki_db.sh .
RUN mkdir rotki-assets && \
    cd rotki-assets && \
    git init && \
    echo "tist" && \
    git remote add origin https://github.com/rotki/assets.git && \
    git fetch origin --depth=1 29038eeba5a7eda3f74cf4bc3eb40169bd9d9d65 && \
    git reset --hard FETCH_HEAD && \
    cd .. && \
    sh ./update_rotki_db.sh


FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y openssl ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/cargo/bin/account-monitor /account-monitor
COPY --from=db-prep /usr/src/app/rotki_db.db /

EXPOSE 3030
ENTRYPOINT ["/account-monitor"]
