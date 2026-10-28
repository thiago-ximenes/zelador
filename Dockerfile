# Duas imagens em uma: o laco e um binario de ~2 MB, mas ele invoca o `claude`, que e
# Node -- entao o runtime precisa do Node de qualquer jeito. O que o Rust economiza aqui
# e o processo de plantao (~3 MB em vez de ~13 MB), nao o tamanho da imagem. Vender o
# contrario seria mentira.
FROM rust:1.90-alpine AS build
RUN apk add --no-cache build-base
WORKDIR /src
COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release

FROM node:24-alpine
# docker-cli: o zelador conversa com o Docker do host pelo socket montado.
# git/openssh: ele commita e abre PR. bash: o que muitos scripts de projeto esperam.
RUN apk add --no-cache docker-cli docker-cli-compose git openssh-client bash ca-certificates curl \
 && npm i -g @anthropic-ai/claude-code@latest \
 && npm cache clean --force
COPY --from=build /src/target/release/zelador /usr/local/bin/zelador
CMD ["zelador"]
