FROM --platform=$TARGETPLATFORM rust:1.65 as builder

RUN USER=root cargo new --bin cfdns

WORKDIR /cfdns

COPY ./Cargo.lock ./Cargo.toml ./conf/ ./.cargo/config.github ./

RUN mkdir .cargo && mv ./config.github ./.cargo/config \
    && cargo build --release \
    && rm src/*.rs

COPY ./src ./src

RUN rm ./target/release/deps/cfdns* \
    && cargo build --release

FROM --platform=$TARGETPLATFORM debian:buster-slim

COPY --from=builder /cfdns/target/release/cfdns ./
COPY ./conf ./conf

CMD ["./cfdns"]