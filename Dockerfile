FROM rust:1-bookworm AS builder

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --release -p fuckipv6-client

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates iproute2 iputils-ping \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -r -u 10001 -s /usr/sbin/nologin -d /var/lib/fuckipv6 fuckipv6 \
    && mkdir -p /etc/fuckipv6 /var/lib/fuckipv6 \
    && chown -R fuckipv6:fuckipv6 /var/lib/fuckipv6

COPY --from=builder /src/target/release/fuckipv6 /usr/local/bin/fuckipv6
COPY examples/node.toml.example /etc/fuckipv6/node.toml

VOLUME ["/etc/fuckipv6", "/var/lib/fuckipv6"]
WORKDIR /var/lib/fuckipv6
USER fuckipv6

ENTRYPOINT ["/usr/local/bin/fuckipv6"]
EXPOSE 8080/tcp 6666/tcp 6667/udp
CMD ["--headless", "--webui-port", "8080"]
