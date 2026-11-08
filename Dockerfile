FROM rust:1-slim AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# Benches are declared in Cargo.toml, so an empty stub keeps manifest parsing happy.
RUN mkdir benches && echo 'fn main() {}' > benches/kv.rs
RUN cargo build --release --bin kvd

FROM debian:stable-slim
COPY --from=build /src/target/release/kvd /usr/local/bin/kvd
VOLUME /data
EXPOSE 7878
ENTRYPOINT ["kvd", "--dir", "/data", "--addr", "0.0.0.0:7878"]
