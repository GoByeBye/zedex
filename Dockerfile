FROM rust:1.73-alpine as builder

WORKDIR /usr/src/zedex
COPY . .

RUN apk add --no-cache \
    musl-dev \
    openssl-dev \
    pkgconf

RUN cargo build --release

FROM alpine:3

RUN apk add --no-cache \
    ca-certificates \
    openssl

WORKDIR /app

COPY --from=builder /usr/src/zedex/target/release/zedex /app/zedex

# Create cache directory
RUN mkdir -p /app/.zedex-cache

# Default port
EXPOSE 2654

# Set the volume for persistent cache
VOLUME ["/app/.zedex-cache"]

# Run with the serve command by default
ENTRYPOINT ["/app/zedex"]
CMD ["serve", "--port", "2654"] 